use std::collections::HashSet;

use crate::{
    ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox, create_overlay_window,
    draw_hints, get_focused_pid, get_main_screen_size, traverse_elements,
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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("Not on main thread");
        let window = create_overlay_window(mtm);
        window.makeKeyAndOrderFront(None);
        let screen_size = get_main_screen_size(mtm);

        Self {
            pressed_keys: HashSet::new(),
            is_active: false,
            hint_boxes: vec![],
            element_cache: ElementCache::new(),
            key_prefix: String::new(),
            screen_size,
            window,
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
        draw_hints(&self.window, &vec![]);
    }

    /// Activates the app and caches UI elements
    pub fn activate(&mut self) {
        if let Some(pid) = get_focused_pid() {
            let focused_window = AXUIElement::application(pid);
            let window_frame = focused_window
                .get_frame()
                .unwrap_or_else(|| Frame::from_origion(self.screen_size));

            self.clear_cache();
            traverse_elements(&focused_window, &window_frame, &mut self.element_cache);

            if !self.element_cache.cache.is_empty() {
                self.is_active = true;

                let new_boxes = self.element_cache.hint_boxes(self.screen_size.height);
                self.hint_boxes.extend(new_boxes);
                draw_hints(&self.window, &self.hint_boxes);
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
                && let Some(ElementOfInterest { element, .. }) = self.element_cache.cache.get(*idx)
            {
                let _ = element.press();
            }
            self.deactivate();
        } else if filtered_boxes.is_empty() {
            self.deactivate();
        } else {
            draw_hints(&self.window, &filtered_boxes);
        }
    }
}
