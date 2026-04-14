use std::collections::HashSet;

use accessibility::{AXUIElement, AXUIElementActions};
use glyphlow::{
    AlphabeticKey, ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox,
    check_accessibility_permissions, create_overlay_window, draw_hints, get_focused_pid,
    get_main_screen_size, traverse_elements,
};
use objc2::MainThreadMarker;
use rdev::{EventType, Key, listen};

fn main() {
    let mtm = MainThreadMarker::new().expect("Not on main thread");
    let window = create_overlay_window(mtm);
    window.makeKeyAndOrderFront(None);
    let screen_size = get_main_screen_size(mtm);

    // Global state to track currently pressed keys
    let mut pressed_keys: HashSet<Key> = HashSet::new();
    // Hint related states
    let mut hint_boxes: Vec<HintBox> = vec![];
    let mut element_cache = ElementCache::new();
    let mut key_prefix = String::new();

    // Check for Accessibility Permissions
    // Note: The app must be granted permission in System Settings > Privacy > Accessibility
    if !check_accessibility_permissions() {
        println!("❌ Error: This app is not trusted. Please grant Accessibility permissions.");
        return;
    }

    let _ = listen(move |event| match event.event_type {
        EventType::KeyPress(key) => {
            // Update global state for mod keys
            pressed_keys.insert(key);
            let key_char = key.to_char();

            if key == Key::Escape {
                key_prefix.clear();
                draw_hints(&window, &vec![]);
            } else if pressed_keys.contains(&Key::Alt)
                // New procedure
                && key == Key::KeyC
                && let Some(pid) = get_focused_pid()
            {
                let focused_window = AXUIElement::application(pid);
                let window_frame = focused_window
                    .get_frame()
                    .unwrap_or_else(|| Frame::from_origion(screen_size));

                element_cache.clear();
                traverse_elements(&focused_window, &window_frame, &mut element_cache);

                let new_boxes = element_cache.hint_boxes(screen_size.height);
                hint_boxes.clear();
                hint_boxes.extend(new_boxes);
                key_prefix.clear();

                draw_hints(&window, &hint_boxes);
            } else if !hint_boxes.is_empty() && key_char != ' ' {
                // Following the hints
                key_prefix.push(key_char);
                let filtered_boxes = hint_boxes
                    .iter()
                    .filter(|b| b.label.starts_with(&key_prefix))
                    .cloned()
                    .collect::<Vec<_>>();

                if filtered_boxes.len() == 1
                    && let Some(HintBox { idx, .. }) = filtered_boxes.first()
                    && let Some(ElementOfInterest { element, .. }) = element_cache.cache.get(*idx)
                {
                    let _ = element.press();
                    key_prefix.clear();
                    draw_hints(&window, &vec![]);
                } else {
                    draw_hints(&window, &filtered_boxes);
                }
            }
        }
        EventType::KeyRelease(key) => {
            pressed_keys.remove(&key);
        }
        _ => {}
    });
}
