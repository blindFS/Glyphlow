use super::{AppEngine, delay};
use crate::{
    Mode,
    ax_element::{ElementOfInterest, GetAttribute, Target, traverse_elements},
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    os_util::{get_focused, get_system_alarm_window},
    util::Frame,
};
use accessibility::AXUIElementAttributes;
use log::Level;
use std::{path::PathBuf, time::Duration};

const SHORT_TIMEOUT: u64 = 1;
const LONG_TIMEOUT: u64 = 2;
const DEBUG_TIMEOUT: u64 = 5;

impl AppEngine {
    pub(super) fn set_mode(&self, mode: Mode) {
        if let Ok(mut state) = self.state.lock() {
            *state = mode;
        }
    }

    pub(super) fn set_simulating_key(&self, flag: bool) {
        if let Ok(mut ks) = self.key_state.lock() {
            ks.is_simulating = flag;
        }
    }

    pub(super) fn check_mode(&self, mode: Mode) -> bool {
        self.state.try_lock().is_ok_and(|s| *s == mode)
    }

    pub(super) fn deactivate(&mut self) {
        self.clear_cache();
        self.clear_drawing();
        self.selected = None;
        self.pending_workflow_actions.clear();
        self.set_mode(Mode::Idle);
    }

    pub(super) fn clear_cache(&mut self) {
        self.word_picker = None;
        self.ocr_cache = None;
        self.notification_layers.clear();
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
        self.multi_selection.reset();
    }

    pub(super) fn notify_then_deactivate(&mut self, msg: &str, log_level: Level) {
        self.set_mode(Mode::WaitAndDeactivate);
        self.notify(msg, log_level);
    }

    pub(super) fn notify(&mut self, msg: &str, log_level: Level) {
        let timeout_secs = match log_level {
            Level::Trace | Level::Info => SHORT_TIMEOUT,
            Level::Debug => DEBUG_TIMEOUT,
            _ => LONG_TIMEOUT,
        };
        log::log!(log_level, "{msg}");
        let notification_layer = self.draw_menu(msg);
        self.notification_layers.push(notification_layer);
        let sender = self.timeout_sender.clone();
        tokio::spawn(async move { delay(sender, timeout_secs).await });
    }

    pub(super) fn select_app_window(
        &mut self,
        vis_level: VisibilityCheckingLevel,
    ) -> Option<Frame> {
        let screen_frame = Frame::from_origion(self.screen_size);

        // NOTE: prioritize system alarms
        if let Some(window) = get_system_alarm_window() {
            let frame = window.get_frame(screen_frame);
            self.last_window_frame = frame;
            self.is_electron = false;

            self.selected = Some(ElementOfInterest::new(
                Some(window),
                None,
                RoleOfInterest::Generic,
                frame,
            ));

            return Some(frame);
        }

        let (pid, focused_app, is_electron) = get_focused()?;
        self.is_electron = is_electron;

        // HACK: need this to bootstrap UI tree generation for some electron apps,
        // e.g. Discord
        if is_electron && (pid != self.last_pid || vis_level == VisibilityCheckingLevel::Loosest) {
            let _ = focused_app.role();
            std::thread::sleep(Duration::from_millis(self.config.electron_initial_wait_ms));
        }
        self.last_pid = pid;

        // HACK: menu items may go out of focused window
        let (focused_window, window_frame) = if vis_level == VisibilityCheckingLevel::Loosest {
            (focused_app, screen_frame)
        } else {
            let mut window = focused_app.focused_window();
            // NOTE: prioritize popover windows, e.g. Apple Music search
            if let Ok(windows) = focused_app.windows()
                && windows.len() > 1
            {
                use accessibility_sys::kAXPopoverRole;
                for win in windows.iter() {
                    if win.role().is_ok_and(|r| r == kAXPopoverRole) {
                        window = Ok(win.clone());
                        break;
                    }
                }
            }
            let window = window.unwrap_or(focused_app);
            let frame = window.get_frame(screen_frame);
            (window, frame)
        };
        self.last_window_frame = window_frame;

        self.selected = Some(ElementOfInterest::new(
            Some(focused_window),
            None,
            RoleOfInterest::Generic,
            window_frame,
        ));

        Some(window_frame)
    }

    pub(super) fn ui_element_traverse_on_activation(&mut self, target: Target) {
        // HACK: abuse self.target to mark whether to call external editor
        self.target = target.clone();
        let target = match target {
            Target::Edit => Target::Editable,
            _ => target,
        };

        let vis_level = match target {
            // NOTE: loose visibility checking for specific targets
            Target::MenuItem | Target::Custom(_) => VisibilityCheckingLevel::Loosest,
            _ => self.config.visibility_checking_level,
        };

        if self.selected.is_none() {
            self.select_app_window(vis_level);
        }

        self.clear_cache();
        if let Some(ElementOfInterest {
            element: Some(element),
            frame,
            ..
        }) = self.selected.as_ref()
        {
            traverse_elements(
                element,
                // Very loose visibility constraint
                frame,
                &self.last_window_frame,
                &mut self.element_cache,
                &target,
                vis_level,
            );
        }
    }

    pub(super) fn activate(&mut self, target: Target) {
        let need_help_msg = target == Target::ChildElement && self.selected.is_none();
        self.ui_element_traverse_on_activation(target);

        if !self.element_cache.cache.is_empty() {
            self.set_mode(Mode::Filtering);
            self.draw_hints_from_cache();
            if need_help_msg {
                self.notify("Press Enter to act.", Level::Trace);
            }
        } else if self.target == Target::Scrollable
            && let Some(eoi) = self.selected.as_ref()
        {
            // Fallback to mouse scroll if no scrollbar found
            let (x, y) = eoi.frame.center();
            self.simulate_event(&rdev::EventType::MouseMove { x, y });
            self.clear_cache();
            self.draw_element_menu("", &RoleOfInterest::ScrollBar, true);
        } else {
            self.clear_drawing();
            self.notify_then_deactivate("No relevant UI elements found.", Level::Warn);
        }
    }

    pub(super) fn handle_file_update(&mut self, pb: PathBuf) {
        if pb == self.temp_file
            && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
        {
            self.update_editing_text(new_text);
        } else if pb != self.temp_file {
            match GlyphlowConfig::load_config(&pb) {
                Ok(mut new_config) => {
                    self.element_cache.reload_config(&new_config);
                    let need_warning = !self.config.safe_reload(&mut new_config);
                    self.config = new_config;

                    if need_warning {
                        self.notify_then_deactivate(
                            "Restart the app to apply full changes",
                            Level::Warn,
                        );
                    } else {
                        self.notify_then_deactivate("Configuration reloaded", Level::Info);
                    }
                }
                Err(msg) => {
                    self.notify_then_deactivate(&msg, Level::Error);
                }
            };
        }
    }
}
