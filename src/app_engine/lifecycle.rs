use super::AppEngine;
use crate::{
    AppSignal, Mode,
    ax_element::{ElementOfInterest, ElementSignal, Target, ThreadSafeElement, traverse},
    config::{GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    os_util::{AppWindowInfo, element_at_point, get_focused_window},
    user_interface::{HintBox, find_overlaps, hint_label_from_index, resolve_collisions},
    util::digits_by_length,
};
use accessibility::AXUIElementAttributes;
use accessibility_sys::{AXUIElementCreateSystemWide, AXUIElementRef};
use core_foundation::base::CFRelease;
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
        self.search_targets.clear();
        self.search_debounce_counter = 0;
        self.is_searching = false;
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
        } * 1000;
        log::log!(log_level, "{msg}");
        let id = self.drawer.notify(msg);
        let sender = self.signal_sender.clone();
        tokio::spawn(
            async move { delay(sender, AppSignal::ClearNotification(id), timeout_secs).await },
        );
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
        // NOTE: make sure filtering mode is set for all kinds of activations
        self.set_mode(Mode::Filtering);

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

    fn hint_visiblility_check(&self, idx: usize, system_wide: AXUIElementRef) -> bool {
        let (x_i, y_i) = self.hint_boxes[idx].frame.center();
        let ele_i = unsafe { element_at_point(system_wide, x_i, y_i) };
        let Some(mut ele_i) = ele_i else {
            return false;
        };
        let cached_ele_i = &self.element_cache.cache[idx];
        cached_ele_i.equals_element(&ele_i)
            // HACK: element at the center of a multi-line static text
            // could be its parent
            || (cached_ele_i.role() == RoleOfInterest::StaticText
                && ele_i
                    .children()
                    .is_ok_and(|children| children.iter().any(|c| cached_ele_i.equals_element(&c))))
            || cached_ele_i.is_ancestor_of(&mut ele_i)
    }

    fn resolve_overlapping(&mut self) {
        let system_wide = unsafe { AXUIElementCreateSystemWide() };
        if system_wide.is_null() {
            return;
        }

        for (i, mut j, i_f) in find_overlaps(&self.hint_boxes, 3.0) {
            let mut confirmed_visible = vec![false; self.hint_boxes.len()];

            if !confirmed_visible[i] && !self.hint_boxes[i].disabled {
                if self.hint_visiblility_check(i, system_wide) {
                    confirmed_visible[i] = true;
                } else {
                    self.hint_boxes[i].fade_out(true);
                    self.hint_boxes[i].disabled = true;
                    continue;
                }
            }

            if !confirmed_visible[j] && !self.hint_boxes[j].disabled {
                if self.hint_visiblility_check(j, system_wide) {
                    confirmed_visible[j] = true;
                } else {
                    self.hint_boxes[j].fade_out(true);
                    self.hint_boxes[j].disabled = true;
                    continue;
                }
            }

            if self.hint_boxes[i].disabled || self.hint_boxes[j].disabled {
                continue;
            }

            // NOTE: One contains the other
            if i_f == self.hint_boxes[i].frame || i_f == self.hint_boxes[j].frame {
                continue;
            }

            // NOTE: Both are visible on their own, check the center of the overlap
            let (x, y) = i_f.center();
            let target_ele = unsafe { element_at_point(system_wide, x, y) };
            let Some(mut target_ele) = target_ele else {
                continue;
            };

            loop {
                if self.element_cache.cache[i].equals_element(&target_ele) {
                    break;
                } else if self.element_cache.cache[j].equals_element(&target_ele) {
                    j = i;
                    break;
                } else if let Ok(parent) = target_ele.parent() {
                    target_ele = parent;
                } else {
                    break;
                }
            }

            self.hint_boxes[j].fade_out(false);
        }
        unsafe { CFRelease(system_wide as *mut _) };
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
            let is_large = w.max(h) >= self.config.colored_frame_min_size as f64;
            if is_large {
                *color_idx += 1;
            };
            let color = (color_num > 0 && is_large)
                .then(|| {
                    self.config
                        .theme
                        .frame_colors
                        .get(*color_idx % color_num)
                        .cloned()
                })
                .flatten();

            let label = hint_label_from_index(idx, None);
            let mut hb = HintBox::new(idx, label, x, y, eoi.frame, color);

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
            if matches!(target, Target::Clickable | Target::Text) {
                self.resolve_overlapping();
            }
            resolve_collisions(&mut self.hint_boxes, self.hint_width, &self.config.theme);
            // Update layers to match final positions and labels without clearing (avoid flicker)
            self.finalize_hints();

            if need_help_msg {
                self.notify("Press Enter to act.", Level::Trace);
            }
        } else if target == Target::ImageOCR {
            // Fallback to full window OCR if no image or leaf group found
            let AppWindowInfo { window, frame, .. } = &self.last_app_window_info;
            let eoi = ElementOfInterest::new(window.clone(), None, RoleOfInterest::Image, *frame);
            self.handle_element_found(eoi, &mut 0, true);
        } else if target == Target::Scrollable
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

pub async fn delay<T>(sender: Sender<T>, id: T, timeout_millis: u64) {
    tokio::time::sleep(Duration::from_millis(timeout_millis)).await;
    let _ = sender.send(id).await;
}
