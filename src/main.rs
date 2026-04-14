use std::collections::HashSet;

use accessibility::AXUIElement;
use glyphlow::{
    GetAttribute, HintBox, StringishElement, check_accessibility_permissions,
    create_overlay_window, draw_hints, get_focused_pid, get_main_screen_height, traverse_elements,
};
use objc2::MainThreadMarker;
use rdev::{EventType, Key, listen};

fn main() {
    let mtm = MainThreadMarker::new().expect("Not on main thread");
    let window = create_overlay_window(mtm);
    let screen_size = get_main_screen_height(mtm);
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

                let mut element_cache: Vec<StringishElement> = vec![];
                traverse_elements(&focused_window, screen_size, &mut element_cache);

                let hint_boxes: Vec<HintBox> = element_cache
                    .iter()
                    .filter_map(|(element, ctx)| {
                        element.center().map(|(x, y)| {
                            HintBox::new(
                                ctx.clone().unwrap_or("A".into()),
                                x,
                                screen_size.height - y,
                            )
                        })
                    })
                    .collect();

                draw_hints(&window, hint_boxes);
            }
        }
        EventType::KeyRelease(key) => {
            pressed_keys.remove(&key);
        }
        _ => {}
    });
}
