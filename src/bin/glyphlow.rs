use core_foundation::{
    base::Boolean,
    runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode},
};
use glyphlow::{
    AppEngine, AppSignal, KeyListener, KeyState, Mode,
    config::{GlyphlowConfig, get_config_path},
    ipc,
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
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixListener,
    sync::mpsc,
};

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
        .as_ref()
        .map_err(|e| e.to_string())
        .and_then(GlyphlowConfig::load_config)
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
    let cache_file = cache_file_path("tempfile.md", true).expect("Failed to create temp file.");
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

    let mut app_engine = AppEngine::new(
        state.clone(),
        key_state.clone(),
        config,
        cache_file,
        tx.clone(),
    );

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

    // Socket file for IPC
    let socket_file =
        cache_file_path(ipc::SOCKET_FILE_NAME, false).expect("Failed to create socket file.");
    let listener = UnixListener::bind(socket_file).expect("Failed to bind socket.");

    loop {
        tokio::select! {
            Ok((stream, _)) = listener.accept() => {
                let mut socket_reader = BufReader::new(stream);
                let mut line = String::new();
                if let Ok(size) = socket_reader.read_line(&mut line).await && size > 0 &&
                    let Ok(signal) = serde_json::from_str::<AppSignal>(&line) {
                    // CLI requests carry no interactive selection, so default to
                    // the focused window as the element of interest. Only do so
                    // when the window was actually resolved, otherwise the info
                    // is the sentinel default (the system-wide element) and a
                    // generic workflow would act on the middle of the screen.
                    if app_engine.get_app_window_info() {
                        app_engine.select_focused_window();
                    }
                    app_engine.handle_signal(signal).await;
                };
            }
            Some(signal) = rx.recv() => app_engine.handle_signal(signal).await,
            Some(pb) = frx.recv() => app_engine.handle_signal(AppSignal::FileUpdate(pb)).await,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                // NOTE: necessary for up-to-date get_focused_pid and UI drawing
                unsafe {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, Boolean::from(false));
                }
            }
        }
    }
}

fn cache_file_path(fname: &str, create: bool) -> Option<PathBuf> {
    let cache_file = ipc::cache_dir()?.join(fname);
    if create {
        log::info!("Creating tempfile: {cache_file:?}");
        std::fs::File::create(&cache_file).ok()?;
    } else {
        let _ = std::fs::remove_file(&cache_file);
    }
    Some(cache_file)
}
