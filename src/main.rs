use std::{collections::HashSet, rc::Rc, sync::Mutex};

use accessibility::{AXUIElement, AXUIElementActions};
use glyphlow::{
    AlphabeticKey, ElementCache, ElementOfInterest, Frame, GetAttribute, HintBox,
    check_accessibility_permissions, create_overlay_window, draw_hints, get_focused_pid,
    get_main_screen_size, traverse_elements,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::NSWindow;
use rdev::{EventType, Key, grab};

#[derive(Default)]
struct AppState {
    pressed_keys: HashSet<Key>,
    is_active: bool,
    hint_boxes: Vec<HintBox>,
    element_cache: ElementCache,
    key_prefix: String,
}

impl AppState {
    fn deactivate(&mut self, window: &Retained<NSWindow>) {
        self.is_active = false;
        self.clear();
        draw_hints(window, &vec![]);
    }

    fn clear(&mut self) {
        self.hint_boxes.clear();
        self.element_cache.clear();
        self.key_prefix.clear();
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("Not on main thread");
    let window = create_overlay_window(mtm);
    window.makeKeyAndOrderFront(None);
    let screen_size = get_main_screen_size(mtm);

    let app_state = Rc::new(Mutex::new(AppState::default()));

    // Check for Accessibility Permissions
    // Note: The app must be granted permission in System Settings > Privacy > Accessibility
    if !check_accessibility_permissions() {
        println!("❌ Error: This app is not trusted. Please grant Accessibility permissions.");
        return;
    }

    let _ = grab(move |event| {
        let mut should_swallow = false;

        let mut cst = app_state.lock().unwrap();

        match event.event_type {
            EventType::KeyPress(key) => {
                // Update global state for mod keys
                cst.pressed_keys.insert(key);
                let key_char = key.to_char();

                if !cst.is_active && cst.pressed_keys.contains(&Key::Alt)
                // New procedure triggered by Alt + C
                // TODO: don't block system events, using Channels instead
                && key == Key::KeyC
                && let Some(pid) = get_focused_pid()
                {
                    cst.is_active = true;
                    should_swallow = true;

                    let focused_window = AXUIElement::application(pid);
                    let window_frame = focused_window
                        .get_frame()
                        .unwrap_or_else(|| Frame::from_origion(screen_size));

                    cst.clear();
                    traverse_elements(&focused_window, &window_frame, &mut cst.element_cache);

                    let new_boxes = cst.element_cache.hint_boxes(screen_size.height);
                    cst.hint_boxes.extend(new_boxes);

                    draw_hints(&window, &cst.hint_boxes);
                } else if cst.is_active {
                    should_swallow = true;

                    // Any key other than A-Z will cancel the operation
                    // TODO: don't block system events, using Channels instead
                    if cst.hint_boxes.is_empty() || key_char == ' ' {
                        cst.deactivate(&window);
                    } else {
                        // Following the hints
                        cst.key_prefix.push(key_char);
                        let filtered_boxes = cst
                            .hint_boxes
                            .iter()
                            .filter(|b| b.label.starts_with(&cst.key_prefix))
                            .cloned()
                            .collect::<Vec<_>>();

                        // Only 1 remaining, click and exit
                        if filtered_boxes.len() == 1 {
                            if let Some(HintBox { idx, .. }) = filtered_boxes.first()
                                && let Some(ElementOfInterest { element, .. }) =
                                    cst.element_cache.cache.get(*idx)
                            {
                                let _ = element.press();
                            }
                            cst.deactivate(&window);
                        } else if filtered_boxes.is_empty() {
                            cst.deactivate(&window);
                        } else {
                            draw_hints(&window, &filtered_boxes);
                        }
                    }
                }
            }
            EventType::KeyRelease(key) => {
                cst.pressed_keys.remove(&key);
                if cst.is_active {
                    should_swallow = true;
                }
            }
            _ => {
                if cst.is_active {
                    should_swallow = true;
                }
            }
        }

        // Return None to swallow the event, Some(event) to pass it to the system
        if should_swallow { None } else { Some(event) }
    });
}
