use std::collections::HashSet;

use crate::{
    action::{dictionary_lookup, text_to_clipboard},
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, RoleOfInterest, Target,
        traverse_elements,
    },
    config::{AlphabeticKey, GlyphlowConfig},
    drawer::{GlyphlowDrawingWindow, create_overlay_window, get_main_screen_size},
    os_util::get_focused_pid,
};
use accessibility::{AXUIElement, AXUIElementActions};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::NSWindow;
use objc2_core_foundation::CGSize;
use rdev::Key;

#[derive(PartialEq)]
enum Mode {
    Idle,
    DashBoard,
    Filtering,
    ActionMenu,
}

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
    window: Retained<NSWindow>,
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

        let config = GlyphlowConfig::load_config();

        Self {
            pressed_keys: HashSet::new(),
            mode: Mode::Idle,
            hint_boxes: vec![],
            element_cache: ElementCache::new(),
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

    fn clear_drawing(&mut self) {
        self.window.clear_window();
    }

    fn draw_hints(&self, boxes: &[HintBox]) {
        self.window.draw_hints(
            boxes,
            &self.config.theme,
            self.key_prefix.len(),
            self.screen_size,
        );
    }

    fn draw_frame_boxes(&self) {
        let frames = self
            .element_cache
            .cache
            .iter()
            .map(|eoi| eoi.frame.invert_y(self.screen_size.height))
            .collect::<Vec<_>>();
        self.window.draw_frame_boxes(&frames);
    }

    fn draw_text_action_menu(&self, text: &str) {
        let mut msg = format!("Select action for text: `{text}`\nCopy (C)\nDictionary (D)");
        for action in self.config.text_actions.iter() {
            msg.push_str(&format!("\n{} ({})", action.display, action.key));
        }
        self.window
            .draw_menu(&msg, self.screen_size, &self.config.theme);
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
                    RoleOfInterest::Group,
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

            let (hint_width, new_boxes) = self.element_cache.hint_boxes(self.screen_size.height);
            self.hint_width = hint_width;
            self.hint_boxes.extend(new_boxes);
            self.draw_hints(&self.hint_boxes);
            if self.target == Target::ChildElement {
                self.draw_frame_boxes();
            }
        } else {
            self.clear_drawing();
        }
    }

    /// Filter the UI elements and redraw hints.
    /// If only 1 remaining, click and exit
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
        if filtered_boxes.len() == 1 {
            if let Some(HintBox { idx, .. }) = filtered_boxes.first()
                && let Some(
                    eoi @ ElementOfInterest {
                        element, context, ..
                    },
                ) = self.element_cache.cache.get(*idx)
            {
                match self.target {
                    Target::Clickable => {
                        if let Err(e) = element.press() {
                            eprintln!("Failed to click element: {e}");
                        };
                        // let _ = element.show_menu();
                        self.deactivate();
                    }
                    Target::Text => {
                        if let Some(text) = context {
                            self.selected = Some(eoi.clone());
                            self.draw_text_action_menu(text);
                            self.mode = Mode::ActionMenu;
                        }
                    }
                    Target::ChildElement => {
                        // eoi.element.inspect();
                        self.selected = Some(eoi.clone());
                        self.activate(&Target::ChildElement);
                        if self.element_cache.cache.is_empty() {
                            // select actions for current selected element
                            // TODO: optimize UX for selected element
                            self.mode = Mode::DashBoard;
                            self.window.draw_menu(
                                "Select Mode:\nClick (C)\nText (T)\nElement (E)",
                                self.screen_size,
                                &self.config.theme,
                            );
                        }
                    }
                }
            }
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            self.draw_hints(&filtered_boxes);
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
                        "Select Mode:\nClick (C)\nText (T)\nElement (E)",
                        self.screen_size,
                        &self.config.theme,
                    );
                    true
                } else {
                    false
                }
            }
            Mode::DashBoard => {
                match key_char {
                    'C' => {
                        self.activate(&Target::Clickable);
                    }
                    'T' => {
                        self.activate(&Target::Text);
                    }
                    'E' => {
                        self.activate(&Target::ChildElement);
                    }
                    _ => {
                        self.deactivate();
                    }
                }
                true
            }
            Mode::Filtering => {
                if key_char == ' ' {
                    self.deactivate();
                } else {
                    self.filter_by_key(key_char);
                }
                true
            }
            Mode::ActionMenu => {
                let Some(ElementOfInterest {
                    context: Some(text),
                    frame,
                    ..
                }) = self.selected.as_ref()
                else {
                    self.deactivate();
                    return true;
                };

                // Chain different actions
                let mut new_text: Option<String> = None;

                // TODO:
                // 1. Multilingual tokenization with charabia
                // 2. URL detection and handling
                match key_char {
                    'C' => {
                        text_to_clipboard(text);
                    }
                    'D' => {
                        if let Some(def_str) = dictionary_lookup(text) {
                            self.window.draw_dictionary_popup(
                                &def_str,
                                &frame.center(),
                                self.screen_size,
                                &self.config.theme,
                            );
                            // HACK: don't call deactivate to close the dictionary window
                            new_text = Some(String::new());
                        }
                    }
                    _ => {
                        for action in &self.config.text_actions {
                            if action.key.to_ascii_uppercase() == key_char {
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
                                        }
                                        if !o.stderr.is_empty() {
                                            eprintln!(
                                                "Stderr: {}",
                                                String::from_utf8_lossy(&o.stderr)
                                            );
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
                }

                if let Some(new_text) = new_text {
                    if !new_text.is_empty()
                        && let Some(selected) = self.selected.as_mut()
                    {
                        selected.context = Some(new_text.clone());
                        self.draw_text_action_menu(&new_text);
                    }
                } else {
                    self.deactivate();
                }
                true
            }
        }
    }
}
