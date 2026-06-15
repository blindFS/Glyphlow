use crate::{
    AppSignal, KeyState, Mode,
    action::{OCRResult, WordPicker, screen_shot, text_from_clipboard},
    ax_element::{ElementCache, ElementOfInterest, Target},
    config::{GlyphlowConfig, RoleOfInterest, WorkFlowAction},
    os_util::AppWindowInfo,
    user_interface::{HintBox, UIDrawer, get_screen_frames},
    util::Frame,
};
use log::Level;
use objc2::MainThreadMarker;
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
    pub(super) one_side_idx: Option<usize>,
    pub(super) role: Option<RoleOfInterest>,
}

impl MultiSeletionState {
    pub(super) fn toggle(&mut self) {
        self.is_on = !self.is_on;
        self.one_side_idx = None;
        self.role = None;
    }

    pub(super) fn reset(&mut self) {
        self.is_on = false;
        self.one_side_idx = None;
        self.role = None;
    }

    pub(super) fn clear_one_side(&mut self) {
        self.one_side_idx = None;
    }

    pub(super) fn set_one_side(&mut self, other: usize) -> Option<(usize, usize)> {
        if let Some(one) = self.one_side_idx {
            Some((one, other))
        } else {
            self.one_side_idx = Some(other);
            None
        }
    }
}

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppEngine {
    pub(super) state: Arc<Mutex<Mode>>,
    pub(super) key_state: Arc<Mutex<KeyState>>,
    pub(super) element_cache: ElementCache,
    pub(super) ocr_cache: Option<OCRResult>,
    pub(super) word_picker: Option<WordPicker>,
    /// Used for drawing hint boxes on screen
    pub(super) hint_boxes: Vec<HintBox>,
    pub(super) hint_prefix: String,
    /// Search related
    pub(super) is_searching: bool,
    pub(super) search_prefix: String,
    pub(super) search_targets: Vec<String>,
    pub(super) search_debounce_counter: usize,
    pub(super) overlay_frame: Frame,
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
    pub(super) signal_sender: Sender<AppSignal>,
    /// Special treatment for Electron based apps.
    /// Like simulate mouse clicking instead of `element.press()`
    pub(super) last_app_window_info: AppWindowInfo,
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
        signal_sender: Sender<AppSignal>,
    ) -> Self {
        let mtm = MainThreadMarker::new().expect("Not on main thread");
        let screen_frames = get_screen_frames(mtm);
        let overlay_frame = Frame::union_of_frames(&screen_frames);
        let drawer = UIDrawer::new(screen_frames, overlay_frame, mtm, &config.theme);

        Self {
            state,
            key_state,
            element_cache: ElementCache::new(
                config.element_min_width as f64,
                config.element_min_height as f64,
                config.image_min_size as f64,
            ),
            ocr_cache: None,
            word_picker: None,
            hint_boxes: vec![],
            hint_prefix: String::new(),
            is_searching: false,
            search_prefix: String::new(),
            search_targets: vec![],
            target: Target::default(),
            hint_width: 0,
            overlay_frame,
            drawer,
            config,
            signal_sender,
            search_debounce_counter: 0,
            selected: None,
            editing: None,
            temp_file,
            last_app_window_info: AppWindowInfo::default(overlay_frame),
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
            AppSignal::MenuRefresh(key_prefix) => {
                self.menu_refresh(&key_prefix, false);
            }
            AppSignal::RunWorkFlow(idx) => {
                self.drawer.clear_menus();
                self.execute_workflow(idx);
            }
            AppSignal::ScrollAction(sa) => {
                self.perform_scroll_action(sa);
            }
            AppSignal::TextAction(ta) => self.perform_text_action(ta),
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
            AppSignal::HintFilter(key_char, mode) => {
                self.filter_by_hint(key_char, mode).await;
            }
            AppSignal::SearchFilter(key_char, mode) => {
                self.filter_by_search(key_char, mode).await;
            }
            AppSignal::SearchDebounce(id, mode) => {
                if self.is_searching && id == self.search_debounce_counter {
                    self.check_filtering(mode).await;
                }
            }
            AppSignal::StartSearch => {
                if self.is_searching {
                    return;
                }
                self.is_searching = true;
                self.drawer.draw_search_bar(&self.search_prefix, true);
                if self.word_picker.is_some() {
                    self.draw_word_picker();
                } else {
                    self.build_search_targets();
                }
            }
            AppSignal::FinishSearch(mode) => {
                self.is_searching = false;
                self.search_debounce_counter = 0;
                self.set_mode(mode.to_app_mode());
                if self.word_picker.is_some() {
                    self.drawer.hide_search_bar();
                } else {
                    self.drawer.clear_menus();
                }
                self.check_filtering(mode).await;
            }
            AppSignal::ActOnEnter => {
                if self.target == Target::ChildElement {
                    // To act on selected parent node
                    self.clear_cache();
                    self.set_mode(Mode::DashBoard);
                    self.menu_refresh("", false);
                }
            }
            AppSignal::ScreenShot => {
                self.clear_cache();
                self.clear_drawing();
                let frame = self
                    .selected
                    .as_ref()
                    .map(|eoi| eoi.frame)
                    .unwrap_or(self.last_app_window_info.frame);
                if screen_shot(&frame).await {
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
                    self.set_mode(Mode::Filtering);
                    self.activate(Target::ImageOCR);
                }
            }
            AppSignal::FileUpdate(pb) => self.handle_file_update(pb),
            AppSignal::ReadClipboard => {
                if let Some(text) = text_from_clipboard() {
                    self.selected = Some(ElementOfInterest::pseudo(
                        None,
                        self.last_app_window_info.frame,
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
