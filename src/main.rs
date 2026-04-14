use std::collections::HashSet;

use accessibility::AXUIElement;
use glyphlow::{
    ElementCache, Frame, GetAttribute, check_accessibility_permissions, create_overlay_window,
    draw_hints, get_focused_pid, get_main_screen_size, traverse_elements,
};
use objc2::MainThreadMarker;
use rdev::{EventType, Key, listen};

fn main() {
    let mtm = MainThreadMarker::new().expect("Not on main thread");
    let window = create_overlay_window(mtm);
    let screen_size = get_main_screen_size(mtm);
    window.makeKeyAndOrderFront(None);

    // Global state to track currently pressed keys
    let mut pressed_keys: HashSet<Key> = HashSet::new();

    println!("--- Glyphlow: Text Element Discovery ---");

    // 1. Check for Accessibility Permissions
    // Note: The app must be granted permission in System Settings > Privacy > Accessibility
    if !check_accessibility_permissions() {
        println!("❌ Error: This app is not trusted. Please grant Accessibility permissions.");
        return;
    }

    let _ = listen(move |event| match event.event_type {
        EventType::KeyPress(key) => {
            pressed_keys.insert(key);
            if key == Key::Escape {
                draw_hints(&window, vec![]);
            } else if pressed_keys.contains(&Key::Alt)
                && key == Key::KeyC
                && let Some(pid) = get_focused_pid()
            {
                let focused_window = AXUIElement::application(pid);
                let window_frame = focused_window
                    .get_frame()
                    .unwrap_or_else(|| Frame::from_origion(screen_size));
                let mut element_cache = ElementCache::new();

                traverse_elements(&focused_window, &window_frame, &mut element_cache);

                let hint_boxes = element_cache.hint_boxes(screen_size.height);
                draw_hints(&window, hint_boxes);
            }
        }
        EventType::KeyRelease(key) => {
            pressed_keys.remove(&key);
        }
        _ => {}
    });
}
