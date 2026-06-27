use glyphlow::{
    AppEngine, AppSignal, FilterMode, KeyListener, KeyState, Mode, ScrollAction, TextAction,
    action::text_to_clipboard, config::GlyphlowConfig,
};
use monio::Key;
use objc2::MainThreadMarker;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
enum TestEvent {
    // Simulates pressing a key (updates KeyState first, then calls KeyListener::key_down)
    PressKey(Key),
    // Simulates releasing a key (updates KeyState)
    ReleaseKey(Key),
    // Directly sets the application Mode (allows isolating tests for specific modes)
    SetMode(Mode),
    // Expects the application to transition to a specific Mode (with timeout)
    ExpectMode(Mode),
    // Expects a specific AppSignal to be handled by the AppEngine (with timeout)
    ExpectSignal(AppSignal),
    // Clears the recorded signals history
    ClearSignals,
    // Sets the clipboard text
    SetClipboard(String),
}

fn main() {
    let _mtm = MainThreadMarker::new().expect("This test must run on the main thread");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        println!("Running Scenario 1: Idle -> Dashboard -> Idle (Deactivation)");
        run_test_scenario(vec![
            TestEvent::ExpectMode(Mode::Idle),
            TestEvent::PressKey(Key::AltLeft),
            TestEvent::PressKey(Key::KeyG),
            TestEvent::ReleaseKey(Key::AltLeft),
            TestEvent::ReleaseKey(Key::KeyG),
            TestEvent::ExpectMode(Mode::DashBoard),
            TestEvent::PressKey(Key::Escape),
            TestEvent::ReleaseKey(Key::Escape),
            TestEvent::ExpectMode(Mode::Idle),
        ])
        .await;

        println!("Running Scenario 2: Filtering Mode Keys");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::Filtering),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::ShiftLeft),
            TestEvent::ExpectSignal(AppSignal::ToggleMultiSelection),
            TestEvent::ReleaseKey(Key::ShiftLeft),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyA),
            TestEvent::ExpectSignal(AppSignal::HintFilter('A', FilterMode::Generic)),
            TestEvent::ReleaseKey(Key::KeyA),
            TestEvent::PressKey(Key::Slash),
            TestEvent::ExpectMode(Mode::Searching(FilterMode::Generic)),
            TestEvent::ExpectSignal(AppSignal::StartSearch),
            TestEvent::ReleaseKey(Key::Slash),
        ])
        .await;

        println!("Running Scenario 3: Searching Mode Keys");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::Searching(FilterMode::Generic)),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyB),
            TestEvent::ExpectSignal(AppSignal::SearchFilter('B', FilterMode::Generic)),
            TestEvent::ReleaseKey(Key::KeyB),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::Enter),
            TestEvent::ExpectSignal(AppSignal::FinishSearch(FilterMode::Generic)),
            TestEvent::ReleaseKey(Key::Enter),
            TestEvent::SetMode(Mode::Searching(FilterMode::Generic)),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::Escape),
            TestEvent::ExpectMode(Mode::Idle),
            TestEvent::ExpectSignal(AppSignal::DeActivate),
            TestEvent::ReleaseKey(Key::Escape),
        ])
        .await;

        println!("Running Scenario 4: Scrolling Mode Keys");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::Scrolling),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyJ),
            TestEvent::ExpectSignal(AppSignal::ScrollAction(ScrollAction::DownRight)),
            TestEvent::ReleaseKey(Key::KeyJ),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyK),
            TestEvent::ExpectSignal(AppSignal::ScrollAction(ScrollAction::UpLeft)),
            TestEvent::ReleaseKey(Key::KeyK),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyI),
            TestEvent::ExpectSignal(AppSignal::ScrollAction(ScrollAction::IncreaseDistance)),
            TestEvent::ReleaseKey(Key::KeyI),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyD),
            TestEvent::ExpectSignal(AppSignal::ScrollAction(ScrollAction::DecreaseDistance)),
            TestEvent::ReleaseKey(Key::KeyD),
            TestEvent::ClearSignals,
            // Prefix building test: press G, then press G again to trigger GG
            TestEvent::PressKey(Key::KeyG),
            TestEvent::ReleaseKey(Key::KeyG),
            TestEvent::PressKey(Key::KeyG),
            TestEvent::ExpectSignal(AppSignal::ScrollAction(ScrollAction::Top)),
            TestEvent::ReleaseKey(Key::KeyG),
        ])
        .await;

        println!("Running Scenario 5: Text Action Menu Mode & Copy Action");
        run_test_scenario(vec![
            TestEvent::SetClipboard("hello text action".to_string()),
            TestEvent::ExpectMode(Mode::Idle),
            TestEvent::PressKey(Key::AltLeft),
            TestEvent::PressKey(Key::KeyG),
            TestEvent::ReleaseKey(Key::AltLeft),
            TestEvent::ReleaseKey(Key::KeyG),
            TestEvent::ExpectMode(Mode::DashBoard),
            TestEvent::PressKey(Key::KeyC), // Read Clipboard -> transitions to TextActionMenu
            TestEvent::ReleaseKey(Key::KeyC),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyC), // Copy action
            TestEvent::ExpectSignal(AppSignal::TextAction(TextAction::Copy)),
            TestEvent::ReleaseKey(Key::KeyC),
        ])
        .await;

        println!("Running Scenario 6: Word Picking Mode & Searching inside Word Picking");
        run_test_scenario(vec![
            TestEvent::SetClipboard("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega one two three".to_string()),
            TestEvent::ExpectMode(Mode::Idle),
            TestEvent::PressKey(Key::AltLeft),
            TestEvent::PressKey(Key::KeyG),
            TestEvent::ReleaseKey(Key::AltLeft),
            TestEvent::ReleaseKey(Key::KeyG),
            TestEvent::ExpectMode(Mode::DashBoard),
            TestEvent::PressKey(Key::KeyC), // Read Clipboard -> transitions to TextActionMenu
            TestEvent::ReleaseKey(Key::KeyC),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::PressKey(Key::KeyS), // Split -> transitions to WordPicking
            TestEvent::ReleaseKey(Key::KeyS),
            TestEvent::ExpectMode(Mode::WordPicking),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyA), // Hint filter
            TestEvent::ExpectSignal(AppSignal::HintFilter('A', FilterMode::WordPicking)),
            TestEvent::ReleaseKey(Key::KeyA),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::Slash), // Start search
            TestEvent::ExpectMode(Mode::Searching(FilterMode::WordPicking)),
            TestEvent::ExpectSignal(AppSignal::StartSearch),
            TestEvent::ReleaseKey(Key::Slash),
        ])
        .await;

        println!("Running Scenario 7: Image Action Menu Mode");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::ImageActionMenu),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyO), // Image OCR action
            TestEvent::ExpectSignal(AppSignal::FrameOCR),
            TestEvent::ReleaseKey(Key::KeyO),
        ])
        .await;

        println!("Running Scenario 8: Dictionary Scrolling Mode");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::DictionaryScrolling),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::Backspace), // Backspace when prefix is empty -> Back to TextActionMenu
            TestEvent::ExpectSignal(AppSignal::BackToTextActionMenu),
            TestEvent::ReleaseKey(Key::Backspace),
        ])
        .await;

        println!("Running Scenario 9: OCR Result Filtering Mode");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::OCRResultFiltering),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyX), // Hint filter in OCR Result Filtering
            TestEvent::ExpectSignal(AppSignal::HintFilter('X', FilterMode::OCR)),
            TestEvent::ReleaseKey(Key::KeyX),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::Slash), // Start search in OCR Result Filtering
            TestEvent::ExpectMode(Mode::Searching(FilterMode::OCR)),
            TestEvent::ExpectSignal(AppSignal::StartSearch),
            TestEvent::ReleaseKey(Key::Slash),
        ])
        .await;

        println!("Running Scenario 10: Wait and Deactivate Mode");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::WaitAndDeactivate),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyA), // Any key should deactivate
            TestEvent::ExpectMode(Mode::Idle),
            TestEvent::ExpectSignal(AppSignal::DeActivate),
            TestEvent::ReleaseKey(Key::KeyA),
        ])
        .await;
    });

    println!("All lifecycle integration tests passed!");
}

async fn run_test_scenario(events: Vec<TestEvent>) {
    // Setup shared state
    let state = Arc::new(Mutex::new(Mode::Idle));
    let key_state = Arc::new(Mutex::new(KeyState::default()));
    let (tx, mut rx) = mpsc::channel::<AppSignal>(100);
    let done = Arc::new(AtomicBool::new(false));
    let processed_signals = Arc::new(Mutex::new(Vec::new()));

    // Load default config
    let config = GlyphlowConfig::default();

    // Create a temporary cache file
    let temp_dir = std::env::temp_dir();
    let cache_file = temp_dir.join("glyphlow_test_tempfile.md");
    if !cache_file.exists() {
        std::fs::File::create(&cache_file).unwrap();
    }

    let key_listener = KeyListener::new(tx.clone(), &config);
    let mut app_engine = AppEngine::new(
        state.clone(),
        key_state.clone(),
        config,
        cache_file.clone(),
        tx.clone(),
    );

    // Run simulator thread concurrently
    let sim_state = state.clone();
    let sim_key_state = key_state.clone();
    let sim_processed_signals = processed_signals.clone();
    let sim_done = done.clone();

    let sim_thread = std::thread::spawn(move || {
        let wait_timeout = Duration::from_millis(1500);

        for (idx, event) in events.into_iter().enumerate() {
            println!("[Sim] Executing event {}: {:?}", idx + 1, event);
            match event {
                TestEvent::PressKey(key) => {
                    sim_key_state.lock().unwrap().key_down(&key);
                    let swallowed =
                        key_listener.key_down(key, &sim_state, &mut sim_key_state.lock().unwrap());
                    println!("[Sim] Key {:?} pressed, swallowed = {}", key, swallowed);
                }
                TestEvent::ReleaseKey(key) => {
                    sim_key_state.lock().unwrap().key_up(&key);
                    println!("[Sim] Key {:?} released", key);
                }
                TestEvent::SetMode(mode) => {
                    *sim_state.lock().unwrap() = mode.clone();
                    println!("[Sim] Mode forced to {:?}", mode);
                }
                TestEvent::ExpectMode(expected_mode) => {
                    let start = std::time::Instant::now();
                    let mut current_mode = sim_state.lock().unwrap().clone();
                    while current_mode != expected_mode && start.elapsed() < wait_timeout {
                        std::thread::sleep(Duration::from_millis(10));
                        current_mode = sim_state.lock().unwrap().clone();
                    }
                    assert_eq!(
                        current_mode, expected_mode,
                        "Assertion failed: expected mode {:?}, but got {:?}",
                        expected_mode, current_mode
                    );
                    println!("[Sim] Confirmed mode matches {:?}", expected_mode);
                }
                TestEvent::ExpectSignal(expected_signal) => {
                    let start = std::time::Instant::now();
                    let mut found = false;
                    while start.elapsed() < wait_timeout {
                        if sim_processed_signals
                            .lock()
                            .unwrap()
                            .contains(&expected_signal)
                        {
                            found = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    assert!(
                        found,
                        "Assertion failed: expected signal {:?} was not processed. Processed signals: {:?}",
                        expected_signal,
                        *sim_processed_signals.lock().unwrap()
                    );
                    println!("[Sim] Confirmed signal {:?} was processed", expected_signal);
                }
                TestEvent::ClearSignals => {
                    sim_processed_signals.lock().unwrap().clear();
                    println!("[Sim] Cleared processed signals history");
                }
                TestEvent::SetClipboard(text) => {
                    text_to_clipboard(&text);
                    println!("[Sim] Clipboard set to {:?}", text);
                }
            }
            // Add a small delay between events to ensure orderly processing
            std::thread::sleep(Duration::from_millis(50));
        }

        sim_done.store(true, Ordering::Relaxed);
    });

    // Main thread event loop
    let loop_timeout = Duration::from_secs(10);
    let start_time = std::time::Instant::now();
    while !done.load(Ordering::Relaxed) {
        if start_time.elapsed() > loop_timeout {
            panic!("Test timed out in main event loop waiting for simulation thread to finish");
        }

        if let Ok(signal) = rx.try_recv() {
            println!("[Main] Processing signal: {:?}", signal);
            processed_signals.lock().unwrap().push(signal.clone());
            app_engine.handle_signal(signal).await;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    sim_thread.join().unwrap();
    let _ = std::fs::remove_file(cache_file);
}
