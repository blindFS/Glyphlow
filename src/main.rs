use core_foundation::{
    base::Boolean,
    runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode},
};
use glyphlow::{
    AppExecutor, AppSignal, KeyListener, Mode,
    config::GlyphlowConfig,
    drawer::{GlyphlowDrawingLayer, create_overlay_window, get_main_screen_size},
    os_util::check_accessibility_permissions,
};
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use objc2::MainThreadMarker;
use objc2_quartz_core::CALayer;
use rdev::{EventType, grab};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    if !check_accessibility_permissions() {
        println!("❌ Error: Accessibility permissions not granted.");
        return;
    }

    let (tx, mut rx) = mpsc::channel::<AppSignal>(100);

    let config = GlyphlowConfig::load_config();
    let key_listener = KeyListener::new(tx, &config);

    let state = Arc::new(Mutex::new(Mode::Idle));
    let pressed_keys = Arc::new(Mutex::new(HashSet::new()));

    let mtm = MainThreadMarker::new().expect("Not on main thread");
    let screen_size = get_main_screen_size(mtm);
    let window = create_overlay_window(mtm, screen_size);
    window.makeKeyAndOrderFront(None);
    let window = CALayer::from_window(&window).expect("Failed to get root layer of window.");

    // Listen to temp file updates
    let temp_file = create_cache_file().expect("Failed to create temp file.");
    let (ftx, mut frx) = mpsc::channel(100);
    // NOTE: listen to file updates with FsEvent
    let Ok(mut debouncer) =
        new_debouncer(
            std::time::Duration::from_millis(200),
            move |res| match res {
                Ok(_) => {
                    // Notify: file updated
                    let _ = ftx.blocking_send(());
                }
                Err(e) => eprintln!("Watch error: {:?}", e),
            },
        )
    else {
        return;
    };

    debouncer
        .watcher()
        .watch(temp_file.as_path(), RecursiveMode::NonRecursive)
        .expect("Failed to watch file.");

    // Listen to notification timeout
    let (ttx, mut trx) = mpsc::channel::<()>(100);
    let mut app_executor =
        AppExecutor::new(state.clone(), config, window, screen_size, temp_file, ttx);

    thread::spawn(move || {
        let pressed_keys = pressed_keys.clone();
        let state = state.clone();
        let _ = grab(move |event| {
            let Ok(mut keys) = pressed_keys.lock() else {
                return Some(event);
            };
            let swallow = match event.event_type {
                EventType::KeyPress(key) => {
                    keys.insert(key);
                    key_listener.key_down(key, &state, &keys)
                }
                EventType::KeyRelease(key) => {
                    keys.remove(&key);
                    key_listener.is_active(&state)
                }
                _ => false,
            };
            (!swallow).then_some(event)
        });
    });

    loop {
        tokio::select! {
            Some(signal) = rx.recv() => app_executor.handle_signal(signal).await,
            Some(()) = frx.recv() => app_executor.handle_signal(AppSignal::FileUpdate).await,
            Some(()) = trx.recv() => app_executor.handle_signal(AppSignal::ClearNotification).await,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                // NOTE: necessary for up-to-date get_focused_pid and UI drawing
                unsafe {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, Boolean::from(false));
                }
            }
        }
    }
}

fn create_cache_file() -> Option<PathBuf> {
    let cache_dir = std::env::var("HOME")
        .ok()
        .map(|dir| PathBuf::from(dir).join(".cache/glyphlow"))?;
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir).ok()?;
    }
    let cache_file = cache_dir.join("tempfile.md");
    Some(cache_file)
}
