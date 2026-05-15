use crate::{
    AppSignal, KeyState, Mode,
    action::{OCRResult, WordPicker, screen_shot, text_from_clipboard},
    ax_element::{ElementCache, ElementOfInterest, Target},
    config::{GlyphlowConfig, RoleOfInterest, WorkFlowAction},
    util::{Frame, HintBox},
};
use log::Level;
use objc2::rc::Retained;
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::Sender;

mod drawing;
mod filtering;
mod interaction;
mod lifecycle;
mod workflow;

pub(crate) static MAX_TEXT_DISPLAY_LEN: usize = 30;

#[derive(Debug, Default)]
pub(crate) struct MultiSeletionState {
    pub(crate) is_on: bool,
    pub(crate) one_side_idex: Option<usize>,
    pub(crate) role: Option<RoleOfInterest>,
}

impl MultiSeletionState {
    pub(crate) fn toggle(&mut self) {
        self.is_on = !self.is_on;
        self.one_side_idex = None;
        self.role = None;
    }

    pub(crate) fn reset(&mut self) {
        self.is_on = false;
        self.one_side_idex = None;
        self.role = None;
    }

    pub(crate) fn clear_one_side(&mut self) {
        self.one_side_idex = None;
    }

    pub(crate) fn set_one_side(&mut self, other: usize) -> Option<(usize, usize)> {
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
    pub(crate) state: Arc<Mutex<Mode>>,
    pub(crate) key_state: Arc<Mutex<KeyState>>,
    /// Used for drawing hint boxes on screen
    pub(crate) hint_boxes: Vec<HintBox>,
    pub(crate) element_cache: ElementCache,
    pub(crate) key_prefix: String,
    pub(crate) screen_size: CGSize,
    pub(crate) window: Retained<CALayer>,
    /// Useful for notification clearing
    pub(crate) notification_layers: Vec<Retained<CALayer>>,
    /// Which elements of interest to look for
    pub(crate) target: Target,
    pub(crate) config: GlyphlowConfig,
    pub(crate) hint_width: u32,
    pub(crate) selected: Option<ElementOfInterest>,
    /// Keep track of editing element,
    /// so that we can use other glyphlow actions while editing
    pub(crate) editing: Option<ElementOfInterest>,
    /// For editing element text values
    pub(crate) temp_file: PathBuf,
    pub(crate) word_picker: Option<WordPicker>,
    pub(crate) ocr_cache: Option<OCRResult>,
    pub(crate) timeout_sender: Sender<()>,
    /// Special treatment for Electron based apps.
    /// Like simulate mouse clicking instead of `element.press()`
    pub(crate) is_electron: bool,
    pub(crate) last_pid: i32,
    pub(crate) last_window_frame: Frame,
    /// For multi-selection
    pub(crate) multi_selection: MultiSeletionState,
    /// Something to finish after filtering
    pub(crate) pending_workflow_actions: VecDeque<WorkFlowAction>,
}

impl AppEngine {
    pub fn new(
        state: Arc<Mutex<Mode>>,
        key_state: Arc<Mutex<KeyState>>,
        config: GlyphlowConfig,
        window: Retained<CALayer>,
        screen_size: CGSize,
        temp_file: PathBuf,
        timeout_sender: Sender<()>,
    ) -> Self {
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
            window,
            notification_layers: Vec::new(),
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
                lifecycle::activate(self, target);
                if quick_follow {
                    filtering::quick_follow(self).await;
                }
            }
            AppSignal::DeActivate => {
                lifecycle::deactivate(self);
            }
            AppSignal::RunWorkFlow(idx) => {
                workflow::execute_workflow(self, idx);
            }
            AppSignal::MenuRefresh(key_prefix) => {
                drawing::menu_refresh(self, &key_prefix, false);
            }
            AppSignal::ToggleMultiSelection => match self.target {
                Target::Text | Target::ImageOCR => {
                    workflow::toggle_multiselection(self);
                }
                _ if self.word_picker.is_some() => {
                    workflow::toggle_multiselection(self);
                }
                _ => {
                    lifecycle::notify(self, "Multi selection only works for text.", Level::Warn);
                }
            },
            AppSignal::Filter(key_char, mode) => {
                filtering::perform_filtering(self, key_char, mode).await;
            }
            AppSignal::ScrollAction(sa) => {
                workflow::perform_scroll_action(self, sa);
            }
            AppSignal::TextAction(ta) => workflow::perform_text_action(self, ta),
            AppSignal::WordPickerStartSearch => {
                if let Some(wp) = self.word_picker.as_mut() {
                    wp.start_searching(self.multi_selection.one_side_idex);
                    self.key_prefix.clear();
                }
            }
            AppSignal::WordPickerFinishSearch => {
                if let Some(wp) = self.word_picker.as_mut() {
                    wp.finish_searching(self.multi_selection.one_side_idex);
                    self.key_prefix = wp.label_prefix.clone();
                }
                filtering::check_word_picker(self);
            }
            AppSignal::ScreenShot => {
                drawing::clear_drawing(self);
                let frame = if let Some(eoi) = self.selected.as_ref() {
                    &eoi.frame
                } else {
                    // Defaults to the window
                    &lifecycle::select_app_window(self, self.config.visibility_checking_level)
                        .unwrap_or_else(|| Frame::from_origion(self.screen_size))
                };
                if screen_shot(frame).await {
                    lifecycle::notify_then_deactivate(
                        self,
                        "Screenshot copied to clipboard.",
                        Level::Info,
                    );
                } else {
                    lifecycle::notify_then_deactivate(
                        self,
                        "Failed to take screenshot.",
                        Level::Error,
                    );
                };
            }
            AppSignal::FrameOCR => {
                if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
                    self.target = Target::ImageOCR;
                    filtering::perform_ocr_on_frame(self, *frame).await;
                } else {
                    drawing::clear_drawing(self);
                    lifecycle::activate(self, Target::ImageOCR);
                }
            }
            AppSignal::FileUpdate(pb) => lifecycle::handle_file_update(self, pb),
            AppSignal::ReadClipboard => {
                drawing::clear_drawing(self);
                if let Some(text) = text_from_clipboard() {
                    self.selected = Some(ElementOfInterest::pseudo(
                        None,
                        Frame::from_origion(self.screen_size),
                    ));
                    workflow::update_selected_text_and_show_menu(self, text);
                } else {
                    lifecycle::notify_then_deactivate(
                        self,
                        "No text found in clipboard.",
                        Level::Warn,
                    );
                }
            }
            AppSignal::ClearNotification => {
                if lifecycle::check_mode(self, Mode::WaitAndDeactivate) {
                    lifecycle::deactivate(self);
                } else {
                    for nl in &self.notification_layers {
                        nl.removeFromSuperlayer();
                    }
                }
                self.notification_layers.clear();
            }
        }
    }
}

pub(crate) async fn delay(sender: Sender<()>, timeout_secs: u64) {
    tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    let _ = sender.send(()).await;
}
