use super::AppEngine;
use crate::{
    Mode,
    ax_element::{ElementOfInterest, ElementSignal, Target, ThreadSafeElement, traverse},
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    os_util::get_focused_window,
    user_interface::{HintBox, hint_label_from_index, resolve_collisions},
    util::digits_by_length,
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
        log::trace!("Set mode: {mode:?}");
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
        self.clear_hints();
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.hint_prefix.clear();
        self.search_prefix.clear();
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
        let id = self.drawer.notify(msg);
        let sender = self.timeout_sender.clone();
        tokio::spawn(async move { delay(sender, id, timeout_secs).await });
    }

    pub(super) fn get_app_window_info(&mut self) {
        let Some(app_win_info) = get_focused_window(
            self.overlay_frame,
            &self.last_app_window_info,
            self.config.electron_initial_wait_ms,
        ) else {
            return;
        };

        self.drawer.select_screen_frame(&app_win_info.frame);
        self.drawer.draw_frame_instant(&app_win_info.frame);
        self.last_app_window_info = app_win_info;
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

        let (focused_only, vis_level) = match target {
            // NOTE: loose visibility checking for specific targets
            Target::Custom(_) => (false, VisibilityCheckingLevel::Loosest),
            _ => (true, self.config.visibility_checking_level),
        };

        if self.selected.is_none() {
            let element =
                if !focused_only && let Ok(ax_app) = self.last_app_window_info.window.parent() {
                    ax_app
                } else {
                    self.last_app_window_info.window.clone()
                };

            self.selected = Some(ElementOfInterest::new(
                element,
                None,
                RoleOfInterest::Generic,
                self.last_app_window_info.frame,
            ));
        }

        let (result_tx, result_rx) = std::sync::mpsc::channel();

        if let Some(selected) = self.selected.as_ref()
            && let Some(element) = selected.element()
        {
            let safe_root = ThreadSafeElement(element.clone());
            let frame = selected.frame;
            let window_frame = if focused_only {
                self.last_app_window_info.frame
            } else {
                self.overlay_frame
            };
            let _ = std::thread::spawn(move || {
                traverse(safe_root, frame, window_frame, target, vis_level, result_tx);
            });
        }

        result_rx
    }

    const HINTBOX_FLUSH_BATCH_SIZE: usize = 5;

    pub(super) fn activate(&mut self, target: Target) {
        log::debug!("Start traversing, target: {target:?}");
        self.clear_cache();
        self.drawer.clear_menus();
        let result_rx = self.ui_element_traverse_on_activation(target);

        let mut color_idx = 0;
        autoreleasepool(|_| {
            for (idx, signal) in result_rx.iter().enumerate() {
                match signal {
                    ElementSignal::ElementFound(Some(ele)) => {
                        let need_flush = (idx + 1) % Self::HINTBOX_FLUSH_BATCH_SIZE == 0;
                        self.handle_element_found(ele, &mut color_idx, need_flush);
                    }
                    ElementSignal::TraversalFinished(target) => {
                        self.handle_traversal_finished(target);
                    }
                    _ => (),
                }
            }
        });
        log::debug!("Finish traversing");
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

            let screen_frame = self.overlay_frame;
            let frame = eoi.frame.intersect(&screen_frame).unwrap_or(screen_frame);

            let (x, y) = frame.center();
            let (w, h) = frame.size();

            let color_num = self.config.theme.frame_colors.len();

            // Draw frames for large enough elements
            let frame = if w.max(h) >= self.config.colored_frame_min_size as f64 {
                *color_idx += 1;
                Some(eoi.frame)
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

            let mut hb = HintBox::new(idx, hint_label_from_index(idx, None), x, y, frame, color);

            hb.draw(
                &self.drawer.root,
                &self.config.theme,
                0,
                &self.overlay_frame,
            );
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
            self.move_mouse_with_trail(x, y);
            self.draw_element_menu("", RoleOfInterest::ScrollBar, true);
        } else if target != Target::ChildElement {
            self.clear_drawing();
            self.notify_then_deactivate("No relevant UI elements found.", Level::Warn);
        }
    }

    pub(super) fn handle_file_update(&mut self, pb: PathBuf) {
        if pb == self.temp_file
            && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
        {
            self.update_editing_text(new_text.trim_end_matches('\n').into());
        } else if pb != self.temp_file {
            match GlyphlowConfig::load_config(&pb) {
                Ok(mut new_config) => {
                    self.element_cache.reload_config(&new_config);
                    let need_warning = !self.config.safe_reload(&mut new_config);
                    self.drawer.reload_theme(&new_config.theme);
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

async fn delay(sender: Sender<usize>, id: usize, timeout_secs: u64) {
    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    let _ = sender.send(id).await;
}
