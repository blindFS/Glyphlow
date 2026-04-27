use crate::{
    AppSignal, DASH_BOARD_MENU_ITEMS, FilterMode, Mode, SCROLLBAR_MENU_ITEMS, ScrollAction,
    StaticMenuItem, TEXT_ACTION_MENU_ITEMS, TextAction,
    action::{
        OCRResult, WordPicker, get_dictionary_attributed_string, perform_ocr, screen_shot,
        text_from_clipboard, text_to_clipboard,
    },
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, RoleOfInterest,
        SetAttribute, Target, hint_boxes_from_frames, traverse_elements,
    },
    config::GlyphlowConfig,
    drawer::GlyphlowDrawingLayer,
    os_util::get_focused_pid,
    util::estimate_frame_for_text,
};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::kAXFocusedAttribute;
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber, string::CFString};

use objc2::rc::Retained;
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;
use rdev::{Button, EventType, simulate};
use tokio::sync::mpsc::Sender;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

static MAX_TEXT_DISPLAY_LEN: usize = 30;

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppExecutor {
    pub state: Arc<Mutex<Mode>>,
    /// Used for drawing hint boxes on screen
    hint_boxes: Vec<HintBox>,
    element_cache: ElementCache,
    key_prefix: String,
    screen_size: CGSize,
    window: Retained<CALayer>,
    /// Which elements of interest to look for
    target: Target,
    config: GlyphlowConfig,
    hint_width: u32,
    selected: Option<ElementOfInterest>,
    /// For editing element text values
    temp_file: PathBuf,
    word_picker: Option<WordPicker>,
    ocr_cache: Option<OCRResult>,
    timeout_sender: Sender<()>,
    /// Special treatment for Electron based apps.
    /// Like simulate mouse clicking instead of `element.press()`
    is_electron: bool,
}

impl AppExecutor {
    pub fn new(
        state: Arc<Mutex<Mode>>,
        config: GlyphlowConfig,
        window: Retained<CALayer>,
        screen_size: CGSize,
        temp_file: PathBuf,
        timeout_sender: Sender<()>,
    ) -> Self {
        Self {
            state,
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
            config,
            timeout_sender,
            selected: None,
            temp_file,
            word_picker: None,
            ocr_cache: None,
            is_electron: false,
        }
    }

    fn set_mode(&self, mode: Mode) {
        if let Ok(mut state) = self.state.lock() {
            *state = mode;
        }
    }

    fn check_mode(&self, mode: Mode) -> bool {
        self.state.try_lock().is_ok_and(|s| *s == mode)
    }

    fn deactivate(&mut self) {
        self.clear_cache();
        self.clear_drawing();
        self.selected = None;
        self.set_mode(Mode::Idle);
    }

    fn clear_cache(&mut self) {
        self.word_picker = None;
        self.ocr_cache = None;
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
    }

    fn clear_drawing(&self) {
        self.window.clear();
    }

    fn draw_selected_frame(&self) {
        if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
            self.window.draw_frame_box(
                &frame.invert_y(self.screen_size.height),
                &self.config.theme.hint_bg_color,
            );
        }
    }

    fn draw_hints(&self, boxes: &[HintBox]) {
        self.clear_drawing();
        self.window.draw_hints(
            boxes,
            &self.config.theme,
            self.key_prefix.len(),
            self.screen_size,
        );
        self.draw_selected_frame();
    }

    fn draw_text_action_menu(&self, text: &str) {
        // Truncate long text
        let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
            &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
        } else {
            text
        };
        let mut msg = "Pick an Action for Text".to_string();
        msg.push_str(&format!("\n\n{}\n", text));
        msg.push_str(&Self::menu_string(&TEXT_ACTION_MENU_ITEMS));
        if let Some(editor) = self.config.editor.as_ref() {
            msg.push_str(&format!("\n{} ({})", editor.display, editor.key));
        }
        for action in self.config.text_actions.iter() {
            msg.push_str(&format!("\n{} ({})", action.display, action.key));
        }
        self.draw_menu(&msg);
    }

    fn draw_scroll_bar_menu(&self) {
        let mut msg = "Pick a Scrolling Action:".to_string();
        msg.push_str(&Self::menu_string(&SCROLLBAR_MENU_ITEMS));
        self.draw_menu(&msg);
    }

    fn draw_dash_board(&self) {
        let mut msg = "Pick a Target:".to_string();
        msg.push_str(&Self::menu_string(&DASH_BOARD_MENU_ITEMS));
        if let Some(editor) = self.config.editor.as_ref() {
            msg.push_str(&format!("\n{} ({})", editor.display, editor.key));
        }
        self.clear_drawing();
        self.draw_selected_frame();
        self.draw_menu(&msg);
    }

    fn draw_menu(&self, msg: &str) {
        self.window
            .draw_menu(msg, self.screen_size, &self.config.theme);
    }

    fn notify(&self, msg: &str) {
        log::info!("{msg}");
        self.draw_menu(msg);
        let sender = self.timeout_sender.clone();
        tokio::spawn(async { delay_and_deactivate(sender).await });
        self.set_mode(Mode::Notification);
    }

    fn menu_string(items: &[StaticMenuItem]) -> String {
        let mut res = String::new();
        for item in items {
            res.push('\n');
            res.push_str(&item.to_string());
        }
        res
    }

    fn draw_word_picker(&self) -> (Vec<String>, u32) {
        let word_picker = self
            .word_picker
            .as_ref()
            .expect("Internal Error: No word picker set.");
        if let Some((text_size, attr_string)) = word_picker.get_attributed_string(
            self.screen_size,
            &self.config.theme,
            &self.key_prefix,
        ) {
            self.window.draw_attributed_string(
                attr_string,
                self.screen_size,
                text_size,
                &self.config.theme,
            );
        };

        (
            word_picker.matched_words(&self.key_prefix),
            word_picker.digits,
        )
    }

    fn select_app_window(&mut self) -> Option<Frame> {
        let (pid, is_electron) = get_focused_pid()?;
        self.is_electron = is_electron;

        let focused_window = AXUIElement::application(pid);
        let window_frame = focused_window
            .get_frame()
            .unwrap_or_else(|| Frame::from_origion(self.screen_size));

        self.selected = Some(ElementOfInterest::new(
            Some(focused_window),
            None,
            RoleOfInterest::GenericNode,
            window_frame.clone(),
        ));
        Some(window_frame)
    }

    fn ui_element_traverse_on_activation(&mut self, target: Target) {
        // HACK: abuse self.target to mark whether to call external editor
        self.target = target.clone();
        let target = match target {
            Target::Edit => Target::Editable,
            _ => target,
        };

        if self.selected.is_none() {
            self.select_app_window();
        }

        self.clear_cache();
        if let Some(ElementOfInterest {
            element: Some(element),
            ..
        }) = self.selected.as_ref()
        {
            traverse_elements(
                element,
                // Very loose visibility constraint
                &Frame::from_origion(self.screen_size),
                &mut self.element_cache,
                &target,
            );
        }
    }

    fn draw_hints_from_cache(&mut self) {
        let (hint_width, new_boxes) = self.element_cache.hint_boxes(
            &Frame::from_origion(self.screen_size),
            &self.config.theme.frame_colors,
            self.config.colored_frame_min_size as f64,
        );
        self.hint_width = hint_width;
        self.hint_boxes = new_boxes;
        self.draw_hints(&self.hint_boxes);
    }

    /// Activates the app and caches UI elements
    fn activate(&mut self, target: Target) {
        self.ui_element_traverse_on_activation(target);

        if !self.element_cache.cache.is_empty() {
            self.draw_hints_from_cache();
            self.set_mode(Mode::Filtering);
        } else {
            self.clear_drawing();
            self.notify("No relevant UI elements found.");
        }
    }

    fn simulate_event(event_type: &EventType) {
        match simulate(event_type) {
            Ok(()) => (),
            Err(e) => {
                log::error!("Failed to simulate event {event_type:?}: {e}");
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    fn simulate_click(x: f64, y: f64) {
        Self::simulate_event(&EventType::MouseMove { x, y });
        Self::simulate_event(&EventType::ButtonPress(Button::Left));
        Self::simulate_event(&EventType::ButtonRelease(Button::Left));
    }

    fn click_on_selected(&self) {
        if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
            let (x, y) = frame.center();
            Self::simulate_click(x, y);
        }
    }

    fn focus_on_element(element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
    }

    fn press_on_element(&self, element: &AXUIElement, role: &RoleOfInterest, center: (f64, f64)) {
        let (x, y) = center;
        Self::focus_on_element(element);

        if self.is_electron || *role == RoleOfInterest::Cell {
            Self::simulate_click(x, y);
        } else if let Err(e) = element.press() {
            log::warn!("Failed to do UI press on element: {e}, simulating mouse click instead...");
            Self::simulate_click(x, y);
        };
    }

    fn scroll_to_value(element: &AXUIElement, val: f64) {
        if let Err(e) = element.set_value(CFNumber::from(val.clamp(0.0, 1.0)).as_CFType()) {
            log::warn!("Failed to set value to the selected scroll bar: {e}.");
        };
    }

    fn right_click_menu_on_element(element: &AXUIElement) {
        if let Err(e) = element.show_menu() {
            log::warn!("Failed to show menu on element: {e}");
        };
    }

    fn ocr_res_filtering(&mut self) {
        let ocr_res = self
            .ocr_cache
            .as_ref()
            .expect("Internal Error: OCR cache not set.");
        let len = ocr_res.len();
        let iter = ocr_res.iter().map(|(_, rect)| Frame::from_cgrect(rect));
        let (digits, ocr_hints) = hint_boxes_from_frames(
            len,
            iter,
            &Frame::from_origion(self.screen_size),
            &self.config.theme.frame_colors,
            self.config.colored_frame_min_size as f64,
        );
        self.hint_width = digits;

        let filtered = ocr_hints
            .into_iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .collect::<Vec<_>>();

        if self.key_prefix.len() == digits as usize
            && let Some(hb) = filtered.first()
        {
            let (selected_text, cg_rect) = ocr_res
                .get(hb.idx)
                .expect("Internal Error: wrong ocr hint indexing.");
            self.selected = Some(ElementOfInterest::new(
                None,
                Some(selected_text.clone()),
                RoleOfInterest::GenericNode,
                Frame::from_cgrect(cg_rect),
            ));
            self.update_selected_text_and_show_menu(selected_text.clone());
        } else if !filtered.is_empty() {
            self.draw_hints(&filtered);
        } else {
            self.deactivate();
        }
    }

    async fn perform_ocr_on_frame(&mut self, frame: Frame) {
        // NOTE: for images with parts out of sight
        let frame = frame
            .intersect(&Frame::from_origion(self.screen_size))
            .unwrap_or(frame);
        match perform_ocr(&frame, &self.config.ocr_languages).await {
            Ok(ocr_res) if !ocr_res.is_empty() => {
                self.ocr_cache = Some(ocr_res);
                self.key_prefix.clear();
                self.ocr_res_filtering();
                self.set_mode(Mode::OCRResultFiltering);
            }
            Err(e) => {
                self.notify(&format!("OCR failed: {e:?}"));
            }
            _ => {
                self.notify("Empty OCR result.");
            }
        }
    }

    /// Filter the UI elements and redraw hints.
    async fn filter_by_key(&mut self) {
        let filtered_boxes = self
            .hint_boxes
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .cloned()
            .collect::<Vec<_>>();

        // Only 1 remaining, take some actions
        if self.key_prefix.len() == self.hint_width as usize
            && filtered_boxes.len() == 1
            && let Some(HintBox { idx, .. }) = filtered_boxes.first()
            && let Some(
                eoi @ ElementOfInterest {
                    element: Some(element),
                    context,
                    frame,
                    role,
                    ..
                },
            ) = self.element_cache.cache.get(*idx)
        {
            // element.inspect();
            self.clear_drawing();
            match self.target {
                Target::MenuItem | Target::Clickable => {
                    let center = frame.center();
                    self.press_on_element(element, role, center);
                    self.deactivate();
                }
                Target::Image => {
                    Self::right_click_menu_on_element(element);
                    // HACK: wait for the right click menu to draw.
                    std::thread::sleep(Duration::from_millis(100));
                    self.activate(Target::MenuItem);
                }
                Target::ImageOCR => self.perform_ocr_on_frame(frame.clone()).await,
                Target::Text => {
                    if let Some(text) = context {
                        self.selected = Some(eoi.clone());
                        self.draw_text_action_menu(text);
                        self.set_mode(Mode::TextActionMenu);
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    self.ui_element_traverse_on_activation(Target::ChildElement);
                    // Actions for current selected element
                    // TODO:
                    // 1. Mouse ops
                    if self.element_cache.cache.is_empty() {
                        self.draw_dash_board();
                        self.set_mode(Mode::DashBoard);
                    } else {
                        self.draw_hints_from_cache();
                    }
                }
                Target::ScrollBar => {
                    self.selected = Some(eoi.clone());
                    self.clear_cache();
                    self.draw_scroll_bar_menu();
                    self.set_mode(Mode::Scrolling);
                }
                Target::Editable => {
                    self.selected = Some(eoi.clone());
                    Self::focus_on_element(element);
                    self.deactivate();
                }
                Target::Edit => {
                    self.selected = Some(eoi.clone());
                    // Focused before editing to increase the success rate
                    Self::focus_on_element(element);
                    let text = context.clone().unwrap_or_default();
                    match self.open_editor(&text) {
                        Ok(_) => {
                            self.set_mode(Mode::Editing);
                        }
                        Err(e) => {
                            self.notify(&format!("Failed to open editor: {e}"));
                        }
                    }
                }
            }
        } else {
            self.draw_hints(&filtered_boxes);
        }
    }

    async fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.key_prefix.push('A');
            self.filter_by_key().await;
        }
    }

    fn open_editor(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let editor = self
            .config
            .editor
            .as_ref()
            .expect("Internal Error: No editor set.");

        // Write current selected text to temp file
        let _ = std::fs::write(&self.temp_file, text);
        let temp_fp = self
            .temp_file
            .to_str()
            .unwrap_or_else(|| panic!("Failed to get temp file path for {:?}.", self.temp_file));

        let args = editor
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_temp_file}", temp_fp));
        let mut child = std::process::Command::new(&editor.command)
            .args(args)
            .spawn()?;

        std::thread::spawn(move || {
            if let Err(e) = child.wait() {
                log::error!("Editor failed to run: {e}");
            }
        });
        Ok(())
    }

    fn take_external_action(&mut self, idx: usize, selected_text: &str) -> bool {
        let action = self
            .config
            .text_actions
            .get(idx)
            .expect("Internal Error: text action idex: {idx} out of bounds.");
        let args = action
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_text}", selected_text));
        let Ok(child) = std::process::Command::new(&action.command)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            self.notify(&format!(
                "Failed to spawn command: {} {}",
                action.command,
                action.args.join(" ")
            ));
            return true;
        };

        // Wait for the stdout as the new text
        match child.wait_with_output() {
            Ok(o) => {
                if !o.stdout.is_empty() {
                    let new_text = String::from_utf8_lossy(&o.stdout)
                        .trim_end_matches('\n')
                        .to_string();
                    self.update_selected_text_and_show_menu(new_text);
                } else if !o.stderr.is_empty() {
                    self.notify(&format!(
                        "External stderr: {}",
                        String::from_utf8_lossy(&o.stderr)
                    ));
                }
            }
            Err(e) => {
                self.notify(&format!("Failed to run command: {e}"));
            }
        }

        true
    }

    fn update_selected_text(&mut self, new_text: String, replace: bool) {
        if let Some(ElementOfInterest {
            element,
            context,
            // role,
            ..
        }) = self.selected.as_mut()
        {
            if replace
                && let Some(ele) = element
                && let Err(e) = ele.set_value(CFString::new(&new_text).as_CFType())
            {
                log::warn!("Failed to set the text of focused element: {element:?}\n Error: {e}");
            }
            *context = Some(new_text);
        }
    }

    fn update_selected_text_and_show_menu(&mut self, new_text: String) {
        self.clear_drawing();
        self.draw_text_action_menu(&new_text);
        self.update_selected_text(new_text, false);
        self.set_mode(Mode::TextActionMenu);
    }

    pub async fn handle_signal(&mut self, signal: AppSignal) {
        match signal {
            AppSignal::DashBoard => {
                if self.check_mode(Mode::Editing) {
                    // Stops editing
                    self.clear_cache();
                    self.selected = None;
                }
                self.draw_dash_board();
                self.set_mode(Mode::DashBoard);
            }
            AppSignal::Activate(target) => {
                let quick_follow = target == Target::ScrollBar
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
            // TODO: Multiple selection
            AppSignal::Filter(key_char, mode) => {
                if key_char == '-' {
                    self.key_prefix.pop();
                } else if self.key_prefix.len() < self.hint_width as usize {
                    self.key_prefix.push(key_char);
                }
                match mode {
                    FilterMode::OCR => {
                        self.ocr_res_filtering();
                    }
                    FilterMode::Generic => {
                        self.filter_by_key().await;
                    }
                    FilterMode::WordPicking => {
                        self.clear_drawing();
                        let (matched_words, digits) = self.draw_word_picker();
                        self.hint_width = digits;

                        if self.key_prefix.len() == digits as usize
                            && matched_words.len() == 1
                            && let Some(text) = matched_words.first()
                        {
                            self.update_selected_text_and_show_menu(text.clone())
                        }
                    }
                }
            }
            AppSignal::ScrollAction(sa) => {
                let Some(ElementOfInterest {
                    element: Some(element),
                    ..
                }) = self.selected.as_ref()
                else {
                    panic!(
                        "A scrollbar is supposed to be selected before entering Mode::Scrolling!"
                    )
                };

                let Some(old_val) = element
                    .value()
                    .ok()
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|f| f.to_f64())
                else {
                    self.deactivate();
                    return;
                };

                let scroll_unit = self.config.scroll_distance;
                match sa {
                    ScrollAction::DownRight => {
                        Self::scroll_to_value(element, old_val + scroll_unit);
                    }
                    ScrollAction::UpLeft => {
                        Self::scroll_to_value(element, old_val - scroll_unit);
                    }
                    ScrollAction::IncreaseDistance => {
                        self.config.scroll_distance *= 1.5;
                    }
                    ScrollAction::DecreaseDistance => {
                        self.config.scroll_distance /= 1.5;
                    }
                }
            }
            AppSignal::TextAction(ta) => {
                let Some(ElementOfInterest {
                    context: Some(text),
                    ..
                }) = self.selected.as_ref()
                else {
                    panic!("Internal Error: No selected text in Mode::TextActionMenu.");
                };

                let text = text.clone();

                // Clear old menu no matter which action is taken
                self.clear_drawing();

                // TODO:
                // 1. URL handling
                let keep_drawing = match ta {
                    TextAction::Press => {
                        self.click_on_selected();
                        false
                    }
                    TextAction::Copy => {
                        text_to_clipboard(&text);
                        self.notify("Copied to clipboard.");
                        true
                    }
                    TextAction::Dictionary => {
                        log::info!("Looking up `{text}` in Apple Dictionary.");
                        if let Some(attr_string) = get_dictionary_attributed_string(
                            &text,
                            &self.config.dictionaries,
                            &self.config.theme,
                        ) {
                            let CGSize { width, height } = self.screen_size;
                            let (text_size, _) =
                                estimate_frame_for_text(&attr_string, (width, height));
                            self.window.draw_attributed_string(
                                attr_string,
                                self.screen_size,
                                text_size,
                                &self.config.theme,
                            );
                        } else {
                            self.notify("No definition found.");
                        }
                        true
                    }
                    TextAction::Split => {
                        let word_picker = WordPicker::new(text);

                        self.clear_cache();
                        self.word_picker = Some(word_picker);
                        self.draw_word_picker();
                        self.set_mode(Mode::WordPicking);
                        true
                    }
                    TextAction::Editor => {
                        if let Err(e) = self.open_editor(&text) {
                            self.notify(&format!("Failed to open editor: {e}"));
                            true
                        } else {
                            false
                        }
                    }
                    TextAction::UserDefined(idx) => self.take_external_action(idx, &text),
                };

                if !keep_drawing {
                    self.deactivate();
                }
            }
            AppSignal::ScreenShot => {
                self.clear_drawing();
                let frame = if let Some(eoi) = self.selected.as_ref() {
                    &eoi.frame
                } else {
                    &self
                        .select_app_window()
                        .map(|f| {
                            // NOTE: Some apps, Finder, returns empty frame
                            if f.size() == (0.0, 0.0) {
                                Frame::from_origion(self.screen_size)
                            } else {
                                f
                            }
                        })
                        .unwrap_or_else(|| Frame::from_origion(self.screen_size))
                };
                if screen_shot(frame).await {
                    self.notify("Screenshot copied to clipboard.");
                } else {
                    self.notify("Failed to take screenshot.");
                };
            }
            AppSignal::FrameOCR => {
                self.clear_drawing();
                if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
                    self.perform_ocr_on_frame(frame.clone()).await;
                } else {
                    self.activate(Target::ImageOCR);
                }
            }
            // TODO: Keep a `self.editing_element` for using other glyphlow features during the editing?
            AppSignal::FileUpdate => {
                if self.check_mode(Mode::Editing)
                    && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
                {
                    self.update_selected_text(new_text, true);
                }
            }
            AppSignal::ReadClipboard => {
                if let Some(text) = text_from_clipboard() {
                    self.draw_text_action_menu(&text);
                    self.selected = Some(ElementOfInterest::new(
                        None,
                        Some(text),
                        RoleOfInterest::GenericNode,
                        Frame::from_origion(self.screen_size),
                    ));
                    self.set_mode(Mode::TextActionMenu);
                } else {
                    self.notify("No text found in clipboard.");
                }
            }
            AppSignal::ClearNotification => {
                if self.check_mode(Mode::Notification) {
                    self.deactivate();
                }
            }
        }
    }
}

async fn delay_and_deactivate(sender: Sender<()>) {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _ = sender.send(()).await;
}
