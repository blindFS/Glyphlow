use std::collections::HashSet;

use crate::{
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, Target, traverse_elements,
    },
    config::{AlphabeticKey, GlyphlowConfig},
    drawer::{
        clear_window, create_overlay_window, draw_dictionary_popup, draw_hints,
        get_main_screen_size,
    },
    os_util::{copy_to_clipboard, dictionary_lookup, get_focused_pid},
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

    pub fn deactivate(&mut self) {
        self.mode = Mode::Idle;
        self.clear_cache();
        self.clear_drawing();
    }

    fn clear_cache(&mut self) {
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
        self.selected = None;
    }

    fn clear_drawing(&mut self) {
        clear_window(&self.window);
    }

    fn draw(&self, boxes: &Vec<HintBox>) {
        draw_hints(
            &self.window,
            boxes,
            &self.config.theme,
            self.key_prefix.len(),
            self.screen_size,
        );
    }

    /// Activates the app and caches UI elements
    pub fn activate(&mut self, target: &Target) {
        self.target = target.clone();

        if let Some(pid) = get_focused_pid() {
            let focused_window = AXUIElement::application(pid);
            let window_frame = focused_window
                .get_frame()
                .unwrap_or_else(|| Frame::from_origion(self.screen_size));

            self.clear_cache();
            traverse_elements(
                &focused_window,
                &window_frame,
                &window_frame,
                &mut self.element_cache,
                target,
            );

            if !self.element_cache.cache.is_empty() {
                self.mode = Mode::Filtering;

                let (hint_width, new_boxes) =
                    self.element_cache.hint_boxes(self.screen_size.height);
                self.hint_width = hint_width;
                self.hint_boxes.extend(new_boxes);
                self.draw(&self.hint_boxes);
            } else {
                self.clear_drawing();
            }
        }
    }

    /// Filter the UI elements and redraw hints.
    /// If only 1 remaining, click and exit
    pub fn follow_key(&mut self, key_char: char) {
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

        // Only 1 remaining, click and exit
        if filtered_boxes.len() == 1 {
            if let Some(HintBox { idx, .. }) = filtered_boxes.first()
                && let Some(eoi @ ElementOfInterest { element, .. }) =
                    self.element_cache.cache.get(*idx)
            {
                if self.target == Target::Clickable {
                    let _ = element.press();
                    // let _ = element.show_menu();
                    self.deactivate();
                } else {
                    self.selected = Some(eoi.clone());
                    // TODO: Draw action menu
                    draw_dictionary_popup(
                        &self.window,
                        "Select Action:\nCopy (C)\nDictionary (D)",
                        &(self.screen_size.width / 2.0, self.screen_size.height / 2.0),
                        self.screen_size,
                        &self.config.theme,
                    );
                    self.mode = Mode::ActionMenu;
                }
            }
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            self.draw(&filtered_boxes);
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
                    // TODO: Draw dashboard
                    draw_dictionary_popup(
                        &self.window,
                        "Select Mode:\nClick (C)\nText (T)",
                        &(self.screen_size.width / 2.0, self.screen_size.height / 2.0),
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
                    self.follow_key(key_char);
                }
                true
            }
            Mode::ActionMenu => {
                if let Some(ElementOfInterest {
                    context: Some(text),
                    center,
                    ..
                }) = self.selected.as_ref()
                {
                    match key_char {
                        'C' => {
                            copy_to_clipboard(text);
                            self.deactivate();
                        }
                        'D' => {
                            if let Some(def_str) = dictionary_lookup(text) {
                                draw_dictionary_popup(
                                    &self.window,
                                    &def_str,
                                    center,
                                    self.screen_size,
                                    &self.config.theme,
                                );
                            } else {
                                // TODO: Logging
                                self.deactivate();
                            }
                        }
                        _ => {
                            self.deactivate();
                        }
                    }
                    true
                } else {
                    self.deactivate();
                    false
                }
            }
        }
    }
}
