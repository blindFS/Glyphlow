use crate::{
    AppSignal, KeyState, Mode,
    action::{OCRResult, WordPicker, screen_shot, text_from_clipboard},
    ax_element::{ElementCache, ElementOfInterest, Target},
    config::{GlyphlowConfig, RoleOfInterest, WorkFlowAction},
    user_interface::{HintBox, UIDrawer, get_main_screen_size},
    util::Frame,
};
use log::Level;
use objc2::MainThreadMarker;
use objc2_core_foundation::CGSize;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::Sender;

mod drawing;
mod filtering;
mod interaction;
mod lifecycle;
mod workflow;

#[derive(Debug, Default)]
pub(super) struct MultiSeletionState {
    pub(super) is_on: bool,
    pub(super) one_side_idex: Option<usize>,
    pub(super) role: Option<RoleOfInterest>,
}

impl MultiSeletionState {
    pub(super) fn toggle(&mut self) {
        self.is_on = !self.is_on;
        self.one_side_idex = None;
        self.role = None;
    }

    pub(super) fn reset(&mut self) {
        self.is_on = false;
        self.one_side_idex = None;
        self.role = None;
    }

    pub(super) fn clear_one_side(&mut self) {
        self.one_side_idex = None;
    }

    pub(super) fn set_one_side(&mut self, other: usize) -> Option<(usize, usize)> {
        if let Some(one) = self.one_side_idex {
            Some((one, other))
        } else {
            self.one_side_idex = Some(other);
            None
        }
    }
}

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppEngine {
    pub(super) state: Arc<Mutex<Mode>>,
    pub(super) key_state: Arc<Mutex<KeyState>>,
    /// Used for drawing hint boxes on screen
    pub(super) hint_boxes: Vec<HintBox>,
    pub(super) element_cache: ElementCache,
    pub(super) key_prefix: String,
    pub(super) screen_size: CGSize,
    pub(super) drawer: UIDrawer,
    /// Which elements of interest to look for
    pub(super) target: Target,
    pub(super) config: GlyphlowConfig,
    pub(super) hint_width: u32,
    pub(super) selected: Option<ElementOfInterest>,
    /// Keep track of editing element,
    /// so that we can use other glyphlow actions while editing
    pub(super) editing: Option<ElementOfInterest>,
    /// For editing element text values
    pub(super) temp_file: PathBuf,
    pub(super) word_picker: Option<WordPicker>,
    pub(super) ocr_cache: Option<OCRResult>,
    pub(super) timeout_sender: Sender<usize>,
    /// Special treatment for Electron based apps.
    /// Like simulate mouse clicking instead of `element.press()`
    pub(super) is_electron: bool,
    pub(super) last_pid: i32,
    pub(super) last_window_frame: Frame,
    /// For multi-selection
    pub(super) multi_selection: MultiSeletionState,
    /// Something to finish after filtering
    pub(super) pending_workflow_actions: VecDeque<WorkFlowAction>,
}

impl AppEngine {
    pub fn new(
        state: Arc<Mutex<Mode>>,
        key_state: Arc<Mutex<KeyState>>,
        config: GlyphlowConfig,
        temp_file: PathBuf,
        timeout_sender: Sender<usize>,
    ) -> Self {
        let mtm = MainThreadMarker::new().expect("Not on main thread");
        let screen_size = get_main_screen_size(mtm);
        let drawer = UIDrawer::new(screen_size, mtm, &config.theme);

        Self {
            state,
            key_state,
            hint_boxes: vec![],
            element_cache: ElementCache::new(
                config.element_min_width as f64,
                config.element_min_height as f64,
                config.image_min_size as f64,
            ),
            key_prefix: String::new(),
            target: Target::default(),
            hint_width: 0,
            screen_size,
            drawer,
            config,
            timeout_sender,
            selected: None,
            editing: None,
            temp_file,
            word_picker: None,
            ocr_cache: None,
            is_electron: false,
            last_pid: 0,
            last_window_frame: Frame::from_origion(screen_size),
            multi_selection: MultiSeletionState::default(),
            pending_workflow_actions: VecDeque::new(),
        }
    }

    pub async fn handle_signal(&mut self, signal: AppSignal) {
        match signal {
            AppSignal::Activate(target) => {
                let quick_follow = target == Target::Scrollable
                    || target == Target::Editable
                    || target == Target::Edit;
                self.activate(target);
                if quick_follow {
                    self.quick_follow().await;
                }
            }
            AppSignal::DeActivate => {
                self.deactivate();
            }
            AppSignal::RunWorkFlow(idx) => {
                self.drawer.clear_menus();
                self.execute_workflow(idx);
            }
            AppSignal::MenuRefresh(key_prefix) => {
                self.menu_refresh(&key_prefix, false);
            }
            AppSignal::ActOnSelected => {
                self.clear_cache();
            }
            AppSignal::ToggleMultiSelection => match self.target {
                Target::Text | Target::ImageOCR => {
                    self.toggle_multiselection();
                }
                _ if self.word_picker.is_some() => {
                    self.toggle_multiselection();
                }
                _ => {
                    self.notify("Multi selection only works for text.", Level::Warn);
                }
            },
            AppSignal::Filter(key_char, mode) => {
                self.perform_filtering(key_char, mode).await;
            }
            AppSignal::ScrollAction(sa) => {
                self.perform_scroll_action(sa);
            }
            AppSignal::TextAction(ta) => self.perform_text_action(ta),
            AppSignal::WordPickerStartSearch => {
                if let Some(wp) = self.word_picker.as_mut() {
                    wp.start_searching(&self.drawer, self.multi_selection.one_side_idex);
                    self.key_prefix.clear();
                }
            }
            AppSignal::WordPickerFinishSearch => {
                if let Some(wp) = self.word_picker.as_mut() {
                    wp.finish_searching(&self.drawer, self.multi_selection.one_side_idex);
                    self.key_prefix = wp.label_prefix.clone();
                }
                self.check_word_picker();
            }
            AppSignal::ScreenShot => {
                self.clear_cache();
                self.clear_drawing();
                let frame = if let Some(eoi) = self.selected.as_ref() {
                    &eoi.frame
                } else {
                    // Defaults to the window
                    &self
                        .select_app_window(self.config.visibility_checking_level)
                        .unwrap_or_else(|| Frame::from_origion(self.screen_size))
                };
                if screen_shot(frame).await {
                    self.notify_then_deactivate("Screenshot copied to clipboard.", Level::Info);
                } else {
                    self.notify_then_deactivate("Failed to take screenshot.", Level::Error);
                };
            }
            AppSignal::FrameOCR => {
                if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
                    self.target = Target::ImageOCR;
                    self.perform_ocr_on_frame(*frame).await;
                } else {
                    self.activate(Target::ImageOCR);
                }
            }
            AppSignal::FileUpdate(pb) => self.handle_file_update(pb),
            AppSignal::ReadClipboard => {
                if let Some(text) = text_from_clipboard() {
                    self.selected = Some(ElementOfInterest::pseudo(
                        None,
                        Frame::from_origion(self.screen_size),
                    ));
                    self.update_selected_text_and_show_menu(text);
                } else {
                    self.notify_then_deactivate("No text found in clipboard.", Level::Warn);
                }
            }
            AppSignal::ClearNotification(id) => {
                if self.check_mode(Mode::WaitAndDeactivate) {
                    self.deactivate();
                } else {
                    self.drawer.clear_notification(id);
                }
            }
        }
    }
}
