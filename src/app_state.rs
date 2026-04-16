use std::collections::HashSet;

use crate::{
    ax_element::{
        ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, Target, traverse_elements,
    },
    config::{GlyphlowConfig, load_config},
    drawer::{
        clear_window, create_overlay_window, draw_hints, get_main_screen_size,
        setup_scrollable_text,
    },
    os_util::{copy_to_clipboard, dictionary_lookup, get_focused_pid},
};
use accessibility::{AXUIElement, AXUIElementActions};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::NSWindow;
use objc2_core_foundation::CGSize;
use rdev::Key;

/// Global state for Glyphlow,
/// mainly cached UI elements, and some related drawings
pub struct AppState {
    /// Keyboard listener for mod keys
    pub pressed_keys: HashSet<Key>,
    pub is_active: bool,
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

        let config = load_config();

        Self {
            pressed_keys: HashSet::new(),
            is_active: false,
            hint_boxes: vec![],
            element_cache: ElementCache::new(),
            key_prefix: String::new(),
            target: Target::default(),
            hint_width: 0,
            screen_size,
            window,
            config,
        }
    }

    pub fn deactivate(&mut self) {
        self.is_active = false;
        self.clear_cache();
        self.clear_drawing();
    }

    fn clear_cache(&mut self) {
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
    }

    fn clear_drawing(&mut self) {
        clear_window(&self.window);
    }

    fn draw(&self, boxes: &Vec<HintBox>) {
        draw_hints(
            &self.window,
            boxes,
            &self.config.theme,
            self.hint_width,
            self.key_prefix.len(),
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
                self.is_active = true;

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
                && let Some(ElementOfInterest {
                    element, context, ..
                }) = self.element_cache.cache.get(*idx)
            {
                if self.target == Target::Clickable {
                    let _ = element.press();
                    // let _ = element.show_menu();
                    self.deactivate();
                } else if let Some(text) = context {
                    if let Some(def_str) = dictionary_lookup(text) {
                        setup_scrollable_text(&self.window, &def_str);
                    }
                    copy_to_clipboard(text);
                }
            }
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            self.draw(&filtered_boxes);
        }
    }
}
