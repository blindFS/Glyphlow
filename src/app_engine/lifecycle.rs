use super::AppEngine;
use crate::{
    Mode,
    ax_element::{
        ElementOfInterest, ElementSignal, GetAttribute, Target, ThreadSafeElement, traverse,
    },
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    os_util::{get_focused, get_system_alarm_window},
    user_interface::{HintBox, hint_label_from_index, resolve_collisions},
    util::{Frame, digits_by_length},
};
use accessibility::AXUIElementAttributes;
use log::Level;
use objc2::rc::autoreleasepool;
use objc2_quartz_core::CATransaction;
use std::{path::PathBuf, sync::mpsc::Receiver, time::Duration};
use tokio::sync::mpsc::Sender;

const SHORT_TIMEOUT: u64 = 1;
const LONG_TIMEOUT: u64 = 2;
const DEBUG_TIMEOUT: u64 = 5;

impl AppEngine {
    pub(super) fn set_mode(&self, mode: Mode) {
        log::log!(Level::Trace, "Set mode: {mode:?}");
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
        self.hint_width = 0;
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
                window,
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
            focused_window,
            None,
            RoleOfInterest::Generic,
            window_frame,
        ));

        Some(window_frame)
    }

    pub(super) fn ui_element_traverse_on_activation(
        &mut self,
        target: Target,
    ) -> Receiver<ElementSignal> {
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
        let (result_tx, result_rx) = std::sync::mpsc::channel();

        if let Some(selected) = self.selected.as_ref()
            && let Some(element) = selected.element()
        {
            let frame = selected.frame;
            let safe_root = ThreadSafeElement(element.clone());
            let window_frame = self.last_window_frame;
            let _ = std::thread::spawn(move || {
                traverse(
                    safe_root,
                    // Very loose visibility constraint
                    frame,
                    window_frame,
                    target,
                    vis_level,
                    result_tx,
                );
            });
        }

        result_rx
    }

    const HINTBOX_FLUSH_BATCH_SIZE: usize = 5;

    pub(super) fn activate(&mut self, target: Target) {
        log::log!(Level::Debug, "Start traversing, target: {target:?}");
        let result_rx = self.ui_element_traverse_on_activation(target);

        self.clear_drawing();
        self.draw_selected_frame();

        let mut color_idx = 0;
        for (idx, signal) in result_rx.iter().enumerate() {
            autoreleasepool(|_| match signal {
                ElementSignal::ElementFound(Some(ele)) => {
                    let need_flush = (idx + 1) % Self::HINTBOX_FLUSH_BATCH_SIZE == 0;
                    self.handle_element_found(ele, &mut color_idx, need_flush);
                }
                ElementSignal::TraversalFinished(target) => {
                    self.handle_traversal_finished(target);
                }
                _ => (),
            })
        }
        log::log!(Level::Debug, "Finish traversing");
    }

    fn handle_element_found(
        &mut self,
        ele: ElementOfInterest,
        color_idx: &mut usize,
        need_flush: bool,
    ) {
        // New element added (not a duplicate)
        if let Some(idx) = self.element_cache.add_by_target(ele, &self.target) {
            let eoi = &self.element_cache.cache[idx];

            let screen_frame = Frame::from_origion(self.screen_size);
            let frame = eoi.frame.intersect(&screen_frame).unwrap_or(screen_frame);

            let (x, y) = frame.center();
            let (w, h) = frame.size();

            let color_num = self.config.theme.frame_colors.len();

            // Draw frames for large enough elements
            let frame = if w.max(h) >= self.config.colored_frame_min_size as f64 {
                *color_idx += 1;
                Some(eoi.frame.invert_y(self.screen_size.height))
            } else {
                None
            };
            let color = (color_num > 0 && frame.is_some())
                .then(|| {
                    self.config
                        .theme
                        .frame_colors
                        .get(*color_idx % color_num)
                        .cloned()
                })
                .flatten();

            let mut hb = HintBox::new(
                idx,
                hint_label_from_index(idx, None),
                x,
                self.screen_size.height - y,
                frame,
                color,
            );

            hb.draw(&self.window, &self.config.theme, 0, self.screen_size);
            if need_flush {
                CATransaction::flush();
            }

            self.hint_boxes.push(hb);
            let digits = digits_by_length(self.hint_boxes.len());

            if digits > self.hint_width {
                for (i, hb) in self.hint_boxes.iter_mut().enumerate() {
                    hb.label = hint_label_from_index(i, Some(digits));
                }
            }
            self.hint_width = digits;
        }
    }

    fn handle_traversal_finished(&mut self, target: Target) {
        let need_help_msg = target == Target::ChildElement && self.selected.is_none();

        if !self.hint_boxes.is_empty() {
            resolve_collisions(&mut self.hint_boxes, self.hint_width, &self.config.theme);
            // Update layers to match final positions and labels without clearing (avoid flicker)
            self.update_hints();

            if need_help_msg {
                self.notify("Press Enter to act.", Level::Trace);
            }
            // For internal activations like workflow action / element explorer
            self.set_mode(Mode::Filtering);
        } else if self.target == Target::Scrollable
            && let Some(eoi) = self.selected.as_ref()
        {
            // Fallback to mouse scroll if no scrollbar found
            let (x, y) = eoi.frame.center();
            self.simulate_event(&rdev::EventType::MouseMove { x, y });
            self.clear_cache();
            self.draw_element_menu("", RoleOfInterest::ScrollBar, true);
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

async fn delay(sender: Sender<()>, timeout_secs: u64) {
    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    let _ = sender.send(()).await;
}
