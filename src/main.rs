use std::{rc::Rc, sync::Mutex};

use glyphlow::{AlphabeticKey, AppState, Target, check_accessibility_permissions};
use rdev::{EventType, Key, grab};

fn main() {
    // Check for Accessibility Permissions
    // Note: The app must be granted permission in System Settings > Privacy > Accessibility
    if !check_accessibility_permissions() {
        println!("❌ Error: This app is not trusted. Please grant Accessibility permissions.");
        return;
    }

    let app_state = Rc::new(Mutex::new(AppState::new()));

    // Hijack key events, if Glyphlow is not active,
    // pass the event back to the system
    let _ = grab(move |event| {
        let mut should_swallow = false;
        let mut cst = app_state.lock().unwrap();

        match event.event_type {
            EventType::KeyPress(key) => {
                // Update global state for mod keys
                cst.pressed_keys.insert(key);
                let key_char = key.to_char();

                if !cst.is_active && cst.pressed_keys.contains(&Key::Alt)
                // New procedure for clickable elements triggered by Alt + C
                && key == Key::KeyC
                {
                    should_swallow = true;
                    // TODO: don't block system events, using Channels instead
                    cst.activate(&Target::Clickable);
                } else if !cst.is_active && cst.pressed_keys.contains(&Key::Alt)
                // New procedure for text elements triggered by Alt + X
                && key == Key::KeyX
                {
                    should_swallow = true;
                    // TODO: don't block system events, using Channels instead
                    cst.activate(&Target::Text);
                } else if cst.is_active {
                    should_swallow = true;

                    // Any key other than A-Z will cancel the operation
                    if key_char == ' ' {
                        cst.deactivate();
                    } else {
                        // TODO: don't block system events, using Channels instead
                        cst.follow_key(key_char);
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
