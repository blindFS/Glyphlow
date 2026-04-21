use std::{rc::Rc, sync::Mutex};

use glyphlow::{AppState, os_util::check_accessibility_permissions};
use rdev::{EventType, grab};

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

        if cst.check_external_output() {
            return Some(event);
        }

        match event.event_type {
            EventType::KeyPress(key) => {
                // Update global state for mod keys
                cst.pressed_keys.insert(key);

                // TODO: don't block system events
                should_swallow = cst.act_on_key(key);
            }
            EventType::KeyRelease(key) => {
                cst.pressed_keys.remove(&key);
                if cst.is_active() {
                    should_swallow = true;
                }
            }
            _ => (),
        }

        // Return None to swallow the event, Some(event) to pass it to the system
        if should_swallow { None } else { Some(event) }
    });
}
