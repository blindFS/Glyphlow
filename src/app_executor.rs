use crate::{
    AppSignal, DASH_BOARD_MENU_ITEMS, FilterMode, Mode, SCROLLBAR_MENU_ITEMS, ScrollAction,
    StaticMenuItem, TEXT_ACTION_MENU_ITEMS, TextAction,
    action::{WordPicker, dictionary_lookup, screen_shot, text_to_clipboard},
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, RoleOfInterest,
        SetAttribute, Target, traverse_elements,
    },
    config::GlyphlowConfig,
    drawer::GlyphlowDrawingLayer,
    os_util::get_focused_pid,
};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::kAXFocusedAttribute;
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber, string::CFString};

use objc2::rc::Retained;
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;

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
}

impl AppExecutor {
    pub fn new(
        state: Arc<Mutex<Mode>>,
        config: GlyphlowConfig,
        window: Retained<CALayer>,
        screen_size: CGSize,
        temp_file: PathBuf,
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
            selected: None,
            temp_file,
            word_picker: None,
        }
    }

    fn set_mode(&self, mode: Mode) {
        if let Ok(mut state) = self.state.lock() {
            *state = mode;
        }
    }

    fn deactivate(&mut self) {
        self.clear_cache();
        self.clear_drawing();
        self.selected = None;
        self.set_mode(Mode::Idle);
    }

    fn clear_cache(&mut self) {
        self.word_picker = None;
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
    }

    fn clear_drawing(&self) {
        self.window.clear();
    }

    fn draw_hints(&self, boxes: &[HintBox]) {
        self.clear_drawing();
        self.window.draw_hints(
            boxes,
            &self.config.theme,
            self.key_prefix.len(),
            self.screen_size,
        );
    }

    fn draw_text_action_menu(&self, text: &str) {
        // Truncate long text
        let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
            &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
        } else {
            text
        };
        let mut msg = Self::menu_string(&TEXT_ACTION_MENU_ITEMS);
        if let Some(editor) = self.config.editor.as_ref() {
            msg.push_str(&format!("\n{} ({})", editor.display, editor.key));
        }
        for action in self.config.text_actions.iter() {
            msg.push_str(&format!("\n{} ({})", action.display, action.key));
        }
        msg.push_str(&format!("\n\n{}", text));
        self.draw_menu(&msg);
    }

    fn draw_menu(&self, msg: &str) {
        self.window
            .draw_menu(msg, self.screen_size, &self.config.theme);
    }

    fn draw_scroll_bar_menu(&self) {
        self.draw_menu(&Self::menu_string(&SCROLLBAR_MENU_ITEMS));
    }

    fn menu_string(items: &[StaticMenuItem]) -> String {
        let mut res = "Pick Action:".to_string();
        for item in items {
            res.push('\n');
            res.push_str(&item.to_string());
        }
        res
    }

    fn draw_dash_board(&self) {
        let mut msg = Self::menu_string(&DASH_BOARD_MENU_ITEMS);
        if let Some(editor) = self.config.editor.as_ref() {
            msg.push_str(&format!("\n{} ({})", editor.display, editor.key));
        }
        self.draw_menu(&msg);
    }

    fn draw_word_picker(&self) -> Vec<String> {
        let word_picker = self
            .word_picker
            .as_ref()
            .expect("Internal Error: No word picker set.");
        let (text_size, attr_string, matched_words) = word_picker.get_attributed_string(
            self.screen_size,
            &self.config.theme.menu_font,
            &self.config.theme.menu_hl_color,
            &self.key_prefix,
        );
        self.window.draw_attributed_string(
            attr_string,
            self.screen_size,
            text_size,
            &self.config.theme,
        );

        matched_words
    }

    fn select_app_window(&mut self) -> Option<Frame> {
        let pid = get_focused_pid()?;
        let focused_window = AXUIElement::application(pid);
        let window_frame = focused_window
            .get_frame()
            .unwrap_or_else(|| Frame::from_origion(self.screen_size));

        self.selected = Some(ElementOfInterest::new(
            focused_window,
            None,
            RoleOfInterest::GenericNode,
            window_frame.clone(),
        ));
        Some(window_frame)
    }

    /// Activates the app and caches UI elements
    fn activate(&mut self, target: Target) {
        // HACK: abuse self.target to mark whether to call external editor
        self.target = target.clone();
        let target = if target == Target::Edit {
            Target::Editable
        } else {
            target
        };

        if self.selected.is_none() {
            self.select_app_window();
        }

        self.clear_cache();
        if let Some(ElementOfInterest { element, .. }) = self.selected.as_ref() {
            traverse_elements(
                element,
                // Very loose visibility constraint
                &Frame::from_origion(self.screen_size),
                &mut self.element_cache,
                &target,
            );
        }

        if !self.element_cache.cache.is_empty() {
            self.set_mode(Mode::Filtering);

            let (hint_width, new_boxes) = self.element_cache.hint_boxes(
                &Frame::from_origion(self.screen_size),
                &self.config.theme.frame_colors,
                self.config.colored_frame_min_size as f64,
            );
            self.hint_width = hint_width;
            self.hint_boxes.extend(new_boxes);
            self.draw_hints(&self.hint_boxes);
        } else {
            // Don't deactivate yet, backspace to rollback
            self.clear_drawing();
        }
    }

    fn focus_on_element(element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
    }

    fn press_on_element(element: &AXUIElement) {
        Self::focus_on_element(element);
        if let Err(e) = element.press() {
            eprintln!("Failed to click element: {e}");
        };
    }

    fn right_click_menu_on_element(element: &AXUIElement) {
        if let Err(e) = element.show_menu() {
            eprintln!("Failed to show menu on element: {e}");
        };
    }

    /// Filter the UI elements and redraw hints.
    fn filter_by_key(&mut self, key_char: char) {
        if key_char == '-' {
            self.key_prefix.pop();
        } else {
            self.key_prefix.push(key_char);
        }

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
                    element, context, ..
                },
            ) = self.element_cache.cache.get(*idx)
        {
            // element.inspect();
            self.clear_drawing();
            match self.target {
                Target::MenuItem | Target::Clickable => {
                    Self::press_on_element(element);
                    self.deactivate();
                }
                // TODO: OCR
                Target::Image => {
                    Self::right_click_menu_on_element(element);
                    // HACK: wait for the right click menu to draw.
                    std::thread::sleep(Duration::from_millis(100));
                    self.activate(Target::MenuItem);
                }
                Target::Text => {
                    if let Some(text) = context {
                        self.selected = Some(eoi.clone());
                        self.draw_text_action_menu(text);
                        self.set_mode(Mode::TextActionMenu);
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    // TODO: optimize UX for selected element
                    // 1. Parent frame
                    // 2. Action menu for parent
                    self.activate(Target::ChildElement);
                    if self.element_cache.cache.is_empty() {
                        // select actions for current selected element
                        // TODO:
                        // 1. Mouse ops
                        self.draw_dash_board();
                        self.set_mode(Mode::DashBoard);
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
                        Ok(_) => (),
                        Err(e) => {
                            eprintln!("Failed to spawn editor process: {e}");
                        }
                    }
                    self.set_mode(Mode::Idle);
                }
            }
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            self.draw_hints(&filtered_boxes);
        }
    }

    fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.filter_by_key('A');
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
                eprintln!("Editor failed to run: {e}");
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
            eprintln!(
                "Failed to spawn command: {} {}",
                action.command,
                action.args.join(" ")
            );
            return false;
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
                    eprintln!("External stderr: {}", String::from_utf8_lossy(&o.stderr));
                    return false;
                }
            }
            Err(e) => {
                eprintln!("Failed to run command: {e}");
                return false;
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
            // if *role == RoleOfInterest::TextField {
            if replace && let Err(e) = element.set_value(CFString::new(&new_text).as_CFType()) {
                eprintln!("Failed to set the text of focused element: {element:?}\n Error: {e}");
            }
            // }
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
            AppSignal::DashBoard => self.draw_dash_board(),
            AppSignal::Activate(target) => {
                let quick_follow = target == Target::ScrollBar
                    || target == Target::Editable
                    || target == Target::Edit;
                self.activate(target);
                if quick_follow {
                    self.quick_follow();
                }
            }
            AppSignal::FileUpdate => {
                if self.target == Target::Edit
                    && let Ok(new_text) = std::fs::read_to_string(&self.temp_file)
                {
                    self.update_selected_text(new_text, true);
                }
            }
            AppSignal::DeActivate => {
                self.deactivate();
            }
            AppSignal::Filter(key_char, mode) => match mode {
                FilterMode::Generic => {
                    self.filter_by_key(key_char);
                }
                FilterMode::WordPicking => {
                    if key_char == '-' {
                        self.key_prefix.pop();
                    } else {
                        self.key_prefix.push(key_char);
                    }

                    self.clear_drawing();
                    let matched_words = self.draw_word_picker();

                    if matched_words.len() == 1
                        && let Some(text) = matched_words.first()
                    {
                        self.update_selected_text_and_show_menu(text.clone())
                    }
                }
            },
            AppSignal::ScreenShot => {
                self.clear_drawing();
                let frame = if let Some(eoi) = self.selected.as_ref() {
                    &eoi.frame
                } else {
                    &self
                        .select_app_window()
                        .unwrap_or_else(|| Frame::from_origion(self.screen_size))
                };
                screen_shot(frame);
                self.deactivate();
            }
            AppSignal::ScrollAction(sa) => {
                let ElementOfInterest { element, .. } = self.selected.as_ref().expect(
                    "A scrollbar is supposed to be selected before entering Mode::Scrolling!",
                );

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
                        let _ = element.set_value(
                            CFNumber::from((old_val + scroll_unit).min(1.0)).as_CFType(),
                        );
                    }
                    ScrollAction::UpLeft => {
                        let _ = element.set_value(
                            CFNumber::from((old_val - scroll_unit).max(0.0)).as_CFType(),
                        );
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
                    panic!("Internal Error: No selected element in Mode::TextActionMenu.");
                };

                let text = text.clone();

                // Clear old menu no matter which action is taken
                self.clear_drawing();

                // TODO:
                // 1. URL handling
                let keep_drawing = match ta {
                    TextAction::Copy => {
                        text_to_clipboard(&text);
                        // TODO: better notification
                        self.draw_menu("Copied to clipboard.");
                        true
                    }
                    TextAction::Dictionary => {
                        // TODO: font size adjustment
                        if let Some(def_str) = dictionary_lookup(&text) {
                            self.draw_menu(&def_str);
                        } else {
                            // TODO: better notification
                            self.draw_menu("No definition found.");
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
                            eprintln!("Failed to open editor: {e}");
                        };
                        false
                    }
                    TextAction::UserDefined(idx) => self.take_external_action(idx, &text),
                };

                if !keep_drawing {
                    self.deactivate();
                }
            }
        }
    }
}
