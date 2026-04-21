use std::collections::HashSet;

use crate::{
    action::{dictionary_lookup, multilingual_split, text_to_clipboard},
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, RoleOfInterest,
        SetAttribute, Target, traverse_elements,
    },
    config::{AlphabeticKey, GlyphlowConfig},
    drawer::{GlyphlowDrawingLayer, create_overlay_window, get_main_screen_size},
    os_util::get_focused_pid,
};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::kAXFocusedAttribute;
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_core_foundation::CGSize;
use objc2_quartz_core::CALayer;
use rdev::Key;

#[derive(PartialEq)]
enum Mode {
    Idle,
    DashBoard,
    Filtering,
    TextActionMenu,
    ElementActionMenu,
    Scrolling,
}

static MAX_TEXT_DISPLAY_LEN: usize = 30;

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppState {
    /// Keyboard listener for mod keys
    pub pressed_keys: HashSet<Key>,
    mode: Mode,
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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("Not on main thread");
        let screen_size = get_main_screen_size(mtm);
        let window = create_overlay_window(mtm, screen_size);
        window.makeKeyAndOrderFront(None);
        let window = CALayer::from_window(&window).expect("Failed to get root layer of window.");

        let config = GlyphlowConfig::load_config();

        Self {
            pressed_keys: HashSet::new(),
            mode: Mode::Idle,
            hint_boxes: vec![],
            element_cache: ElementCache::new(
                config.element_min_width as f64,
                config.element_min_height as f64,
            ),
            key_prefix: String::new(),
            target: Target::default(),
            hint_width: 0,
            screen_size,
            window,
            config,
            selected: None,
        }
    }

    fn deactivate(&mut self) {
        self.mode = Mode::Idle;
        self.clear_cache();
        self.selected = None;
        self.clear_drawing();
    }

    fn clear_cache(&mut self) {
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
        let mut msg = format!("Select action for text:\n\n{text}\n\n⮺ Copy (C)\n◫ Dictionary (D)");
        for action in self.config.text_actions.iter() {
            msg.push_str(&format!("\n{} ({})", action.display, action.key));
        }
        self.window
            .draw_menu(&msg, self.screen_size, &self.config.theme);
    }

    fn draw_element_action_menu(&self) {
        self.window.draw_menu(
            "Select Mode:\n\nText (T)\nPress (P)\nScrollBar (S)",
            self.screen_size,
            &self.config.theme,
        );
    }

    /// Activates the app and caches UI elements
    fn activate(&mut self, target: &Target) {
        self.target = target.clone();

        if self.selected.is_none() {
            self.selected = get_focused_pid().map(|pid| {
                let focused_window = AXUIElement::application(pid);
                let window_frame = focused_window
                    .get_frame()
                    .unwrap_or_else(|| Frame::from_origion(self.screen_size));

                ElementOfInterest::new(
                    focused_window,
                    None,
                    RoleOfInterest::GenericNode,
                    window_frame.clone(),
                )
            });
        }

        self.clear_cache();
        if let Some(ElementOfInterest { element, .. }) = self.selected.as_ref() {
            traverse_elements(
                element,
                // Very loose visibility constraint
                &Frame::from_origion(self.screen_size),
                &mut self.element_cache,
                target,
            );
        }

        if !self.element_cache.cache.is_empty() {
            self.mode = Mode::Filtering;

            let (hint_width, new_boxes) = self.element_cache.hint_boxes(
                &Frame::from_origion(self.screen_size),
                &self.config.theme.frame_colors,
                self.config.colored_frame_min_size as f64,
            );
            self.hint_width = hint_width;
            self.hint_boxes.extend(new_boxes);
            self.draw_hints(&self.hint_boxes);
        } else {
            self.clear_drawing();
        }
    }

    fn press_on_element(element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
        if let Err(e) = element.press() {
            eprintln!("Failed to click element: {e}");
        };
        // let _ = element.show_menu();
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
            // eoi.element.inspect();
            self.clear_drawing();
            match self.target {
                Target::Clickable => {
                    Self::press_on_element(element);
                    self.deactivate();
                }
                Target::Text => {
                    if let Some(text) = context {
                        self.selected = Some(eoi.clone());
                        self.draw_text_action_menu(text);
                        self.mode = Mode::TextActionMenu;
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    // TODO: optimize UX for selected element
                    // 1. Parent frame
                    // 2. Action menu for parent
                    self.activate(&Target::ChildElement);
                    if self.element_cache.cache.is_empty() {
                        // select actions for current selected element
                        // TODO:
                        // 1. Screen shot
                        // 2. Mouse ops
                        self.draw_element_action_menu();
                        self.mode = Mode::ElementActionMenu;
                    }
                }
                Target::ScrollBar => {
                    self.selected = Some(eoi.clone());
                    self.clear_cache();
                    self.window.draw_menu(
                            "Scroll With Following Keys:\n\nDown/Right (J)\nUp/Left (K)\nDistance Increase (I)\nDistance Decrease (D)",
                            self.screen_size,
                            &self.config.theme,
                        );
                    self.mode = Mode::Scrolling;
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

    pub fn is_active(&self) -> bool {
        self.mode != Mode::Idle
    }

    pub fn act_on_key(&mut self, key: Key) -> bool {
        let key_char = key.to_char();

        match self.mode {
            Mode::Idle => {
                if self.config.global_trigger_key.keys.iter().all(|k| {
                    k == &key
                        || self.pressed_keys.contains(k)
                        || k.right_alternative()
                            .is_some_and(|r| *k == r || self.pressed_keys.contains(&r))
                }) {
                    self.mode = Mode::DashBoard;
                    self.window.draw_menu(
                        "Select Mode:\n\nPress (P)\nText (T)\nElement (E)\nScrollBar (S)",
                        self.screen_size,
                        &self.config.theme,
                    );
                    true
                } else {
                    false
                }
            }
            // TODO: Mode::Input for input text fields/areas
            Mode::DashBoard => {
                match key_char {
                    'P' => {
                        self.activate(&Target::Clickable);
                    }
                    'T' => {
                        self.activate(&Target::Text);
                    }
                    'E' => {
                        self.activate(&Target::ChildElement);
                    }
                    'S' => {
                        self.activate(&Target::ScrollBar);
                        self.quick_follow();
                    }
                    _ => {
                        self.deactivate();
                    }
                }
                true
            }
            Mode::Filtering => {
                // NOTE: Act on currently selected parent node
                if key == Key::Return && self.selected.is_some() {
                    self.draw_element_action_menu();
                    self.mode = Mode::ElementActionMenu;
                } else if key_char == ' ' {
                    self.deactivate();
                } else {
                    self.filter_by_key(key_char);
                }
                true
            }
            Mode::ElementActionMenu => {
                match key_char {
                    'P' => {
                        self.activate(&Target::Clickable);
                    }
                    'T' => {
                        self.activate(&Target::Text);
                    }
                    'S' => {
                        self.activate(&Target::ScrollBar);
                    }
                    _ => {
                        self.deactivate();
                        return false;
                    }
                }
                self.quick_follow();
                true
            }
            Mode::TextActionMenu => {
                let Some(ElementOfInterest {
                    context: Some(text),
                    ..
                }) = self.selected.as_ref()
                else {
                    self.deactivate();
                    return true;
                };

                // Chain different actions
                let mut new_text: Option<String> = None;
                let mut keep_drawing = false;

                // Clear old menu no matter which action is taken
                self.clear_drawing();

                // TODO:
                // 1. URL handling
                match key_char {
                    'C' => {
                        text_to_clipboard(text);
                        // TODO: better notification
                        self.window.draw_menu(
                            "Copied to clipboard.",
                            self.screen_size,
                            &self.config.theme,
                        );
                        keep_drawing = true;
                    }
                    'D' => {
                        if let Some(def_str) = dictionary_lookup(text) {
                            self.window
                                .draw_menu(&def_str, self.screen_size, &self.config.theme);
                        } else {
                            // TODO: better notification
                            self.window.draw_menu(
                                "No definition found.",
                                self.screen_size,
                                &self.config.theme,
                            );
                        }
                        keep_drawing = true;
                    }
                    'S' => {
                        let words = multilingual_split(text);
                        self.window.draw_menu(
                            &words.join(" "),
                            self.screen_size,
                            &self.config.theme,
                        );
                        keep_drawing = true;
                    }
                    _ => {
                        for action in &self.config.text_actions {
                            if action.key.to_ascii_uppercase() != key_char {
                                continue;
                            }
                            match std::process::Command::new(&action.command)
                                .args(
                                    action
                                        .args
                                        .iter()
                                        .map(|arg| arg.replace("{selection}", text)),
                                )
                                .stdout(std::process::Stdio::piped())
                                .spawn()
                                .and_then(|child| child.wait_with_output())
                            {
                                Ok(o) => {
                                    if !o.stdout.is_empty() {
                                        new_text = Some(
                                            String::from_utf8_lossy(&o.stdout)
                                                .trim_end_matches('\n')
                                                .to_string(),
                                        );
                                        keep_drawing = true;
                                    }
                                    if !o.stderr.is_empty() {
                                        eprintln!("Stderr: {}", String::from_utf8_lossy(&o.stderr));
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to run command: {e}");
                                }
                            };
                            break;
                        }
                    }
                }

                if let Some(new_txt) = new_text
                    && !new_txt.is_empty()
                    && let Some(selected) = self.selected.as_mut()
                {
                    selected.context = Some(new_txt.clone());
                    self.draw_text_action_menu(&new_txt);
                }

                if !keep_drawing {
                    self.deactivate();
                }

                true
            }
            Mode::Scrolling => {
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
                    return false;
                };

                let scroll_unit = self.config.scroll_distance;
                match key_char {
                    'J' => {
                        let _ = element.set_value(
                            CFNumber::from((old_val + scroll_unit).min(1.0)).as_CFType(),
                        );
                    }
                    'K' => {
                        let _ = element.set_value(
                            CFNumber::from((old_val - scroll_unit).max(0.0)).as_CFType(),
                        );
                    }
                    'I' => {
                        self.config.scroll_distance *= 1.5;
                    }
                    'D' => {
                        self.config.scroll_distance /= 1.5;
                    }
                    _ => {
                        self.deactivate();
                    }
                }
                true
            }
        }
    }
}
