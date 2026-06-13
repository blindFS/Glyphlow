use core_foundation::{
    base::Boolean,
    runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode},
};
use glyphlow::{
    AppEngine, AppSignal, KeyListener, KeyState, Mode,
    config::{GlyphlowConfig, get_config_path},
    os_util::check_accessibility_permissions,
};
use monio::{EventType, grab};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .env()
        .init()
        .expect("Failed to init logger");

    if !check_accessibility_permissions() {
        log::error!("Accessibility permissions not granted.");
        return;
    }

    let (tx, mut rx) = mpsc::channel::<AppSignal>(1000);

    let config_path = get_config_path();
    let config = match config_path
        .clone()
        .and_then(|cp| GlyphlowConfig::load_config(&cp))
    {
        Ok(config) => config,
        Err(msg) => {
            log::error!("{msg}");
            GlyphlowConfig::default()
        }
    };

    log::info!(
        "Press key combination {:?} to start",
        config.global_trigger_key.keys
    );

    let key_listener = KeyListener::new(tx.clone(), &config);

    let state = Arc::new(Mutex::new(Mode::Idle));
    // Key state for tracking pressed keys and simulating state
    let key_state = Arc::new(Mutex::new(KeyState::default()));

    // Listen to temp file updates
    let cache_file = create_cache_file().expect("Failed to create temp file.");
    let (ftx, mut frx) = mpsc::channel::<PathBuf>(100);
    // NOTE: listen to file updates with FsEvent
    let Ok(mut debouncer) = new_debouncer(
        std::time::Duration::from_millis(200),
        move |res: DebounceEventResult| match res {
            Ok(events) => {
                let mut pbs: HashSet<PathBuf> = HashSet::new();
                for e in events {
                    pbs.insert(e.path);
                }
                for pb in pbs {
                    let _ = ftx.blocking_send(pb);
                }
            }
            Err(e) => log::error!("Watch error: {:?}", e),
        },
    ) else {
        log::error!("Failed to create debouncer.");
        return;
    };

    if let Err(e) = debouncer
        .watcher()
        .watch(cache_file.as_path(), RecursiveMode::NonRecursive)
    {
        log::error!("Failed to watch temp file: {e}");
    }
    if let Ok(path) = config_path
        && let Err(e) = debouncer
            .watcher()
            .watch(path.as_path(), RecursiveMode::NonRecursive)
    {
        log::error!("Failed to watch config file: {e}");
    }

    // Listen to notification timeout
    let (ttx, mut trx) = mpsc::channel::<usize>(100);
    let mut app_engine = AppEngine::new(state.clone(), key_state.clone(), config, cache_file, ttx, tx.clone());

    thread::spawn(move || {
        let key_state = key_state.clone();
        let state = state.clone();
        let _ = grab(move |event| {
            let Ok(mut k_s) = key_state.lock() else {
                return Some(event.clone());
            };
            if k_s.is_simulating {
                return Some(event.clone());
            }

            let mut pass_on = true;
            if let Some(kb) = &event.keyboard {
                match event.event_type {
                    EventType::KeyPressed => {
                        k_s.key_down(&kb.key);
                        pass_on = !key_listener.key_down(kb.key, &state, &mut k_s)
                    }
                    EventType::KeyReleased => k_s.key_up(&kb.key),
                    _ => (),
                }
            };
            pass_on.then(|| event.clone())
        });
    });

    loop {
        tokio::select! {
            Some(signal) = rx.recv() => app_engine.handle_signal(signal).await,
            Some(pb) = frx.recv() => app_engine.handle_signal(AppSignal::FileUpdate(pb)).await,
            Some(id) = trx.recv() => app_engine.handle_signal(AppSignal::ClearNotification(id)).await,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                // NOTE: necessary for up-to-date get_focused_pid and UI drawing
                unsafe {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, Boolean::from(false));
                }
            }
        }
    }
}

fn create_cache_file() -> Option<PathBuf> {
    let cache_dir = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|dir| PathBuf::from(dir).join("glyphlow"))
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|dir| PathBuf::from(dir).join(".cache/glyphlow"))
        })?;
    if !cache_dir.exists() {
        std::fs::create_dir_all(&cache_dir).ok()?;
    }
    let cache_file = cache_dir.join("tempfile.md");
    if !cache_file.exists() {
        log::info!("Creating tempfile: {cache_file:?}");
        std::fs::File::create(&cache_file).ok()?;
    }
    Some(cache_file)
}
