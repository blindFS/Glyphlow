use glyphlow::{
    AppEngine, AppSignal, FilterMode, KeyListener, KeyState, Mode, ModifierKey, ScrollAction,
    TextAction,
    action::text_to_clipboard,
    ax_element::Target,
    config::{GlyphlowConfig, RoleOfInterest, WorkFlow},
};
use monio::Key;
use objc2::MainThreadMarker;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc;

/// Five words, so the default alphabet gives every word a single-key label:
/// `alpha` = `A`, `beta` = `B`, `gamma` = `C`, `delta` = `D`, `zeta` = `E`.
const FIVE_WORDS: &str = "alpha beta gamma delta zeta";

/// Notifications the engine showed, in order.
///
/// What a modifier tap did is not visible on the signal channel — it shows up
/// only as a notification — so the log is where a test can read it back.
static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct MessageLog;

impl log::Log for MessageLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Ok(mut messages) = MESSAGES.lock() {
            messages.push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static MESSAGE_LOG: MessageLog = MessageLog;

/// A single step the simulator thread performs.
///
/// The `Expect*` events poll with a timeout, so a transition that never happens
/// fails the step instead of hanging the whole run.
#[derive(Debug, Clone)]
enum TestEvent {
    /// Updates `KeyState` first, then calls `KeyListener::key_down`.
    PressKey(Key),
    ReleaseKey(Key),
    SetMode(Mode),
    ExpectMode(Mode),
    ExpectSignal(AppSignal),
    ClearSignals,
    SetClipboard(String),
    /// Sends a raw signal, the way the CLI would over its socket.
    SendSignal(AppSignal),
    /// Drops the notifications captured so far.
    ClearMessages,
    /// Waits for a notification whose text is exactly this.
    ExpectMessage(String),
    /// Waits as long, and fails if anything was notified at all.
    ExpectNoMessage,
}

fn main() {
    let _mtm = MainThreadMarker::new().expect("This test must run on the main thread");

    log::set_logger(&MESSAGE_LOG).expect("the test binary owns the logger");
    log::set_max_level(log::LevelFilter::Info);

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
            TestEvent::ExpectSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
            TestEvent::ReleaseKey(Key::ShiftLeft),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::ControlLeft),
            TestEvent::ExpectSignal(AppSignal::ToggleModifier(ModifierKey::Ctrl)),
            TestEvent::ReleaseKey(Key::ControlLeft),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::AltLeft),
            TestEvent::ExpectSignal(AppSignal::ToggleModifier(ModifierKey::Alt)),
            TestEvent::ReleaseKey(Key::AltLeft),
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

        println!("Running Scenario 11: CLI driven workflow by name");
        // These bypass the server's focused-window pre-selection (the harness
        // has no accessibility tree), so only paths that do not depend on the
        // current selection are asserted here. The role-based "offer elements
        // for picking" path needs a live accessibility tree and is not covered.
        let mut cli_config = GlyphlowConfig::default();
        cli_config.workflows.push(WorkFlow {
            display: "Other App Only".into(),
            key: "Z".into(),
            valid_app_ids: Some(vec!["com.example.not-the-focused-app".into()]),
            starting_role: RoleOfInterest::Any,
            actions: vec![],
        });
        run_test_scenario_with_config(
            vec![
                TestEvent::SetMode(Mode::Idle),
                TestEvent::ClearSignals,
                // An unknown name must be reported, not silently ignored
                TestEvent::SendSignal(AppSignal::RunWorkFlowByName("no such workflow".into())),
                TestEvent::ExpectMode(Mode::WaitAndDeactivate),
                TestEvent::SetMode(Mode::Idle),
                TestEvent::ClearSignals,
                // Restricted to another app: refused even though `Any` would
                // otherwise be satisfiable, because the app check is a hard no.
                TestEvent::SendSignal(AppSignal::RunWorkFlowByName("Other App Only".into())),
                TestEvent::ExpectMode(Mode::WaitAndDeactivate),
            ],
            cli_config,
        )
        .await;

        println!("Running Scenario 12: Backspace leaves word picking for the text action menu");
        run_test_scenario(vec![
            TestEvent::SetClipboard(FIVE_WORDS.to_string()),
            TestEvent::SendSignal(AppSignal::ReadClipboard),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::SendSignal(AppSignal::TextAction(TextAction::Split)),
            TestEvent::ExpectMode(Mode::WordPicking),
            // Nothing typed yet, so there is nothing to pop: backspace is the
            // "go back one level" key.
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::ExpectMode(Mode::TextActionMenu),
        ])
        .await;

        println!("Running Scenario 13: Backspace pops the hint key, then the search filter");
        run_test_scenario(vec![
            TestEvent::SetClipboard(FIVE_WORDS.to_string()),
            TestEvent::SendSignal(AppSignal::ReadClipboard),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::SendSignal(AppSignal::TextAction(TextAction::Split)),
            TestEvent::ExpectMode(Mode::WordPicking),
            // Search for "et": it matches beta and zeta, but not alpha.
            TestEvent::PressKey(Key::Slash),
            TestEvent::ReleaseKey(Key::Slash),
            TestEvent::PressKey(Key::KeyE),
            TestEvent::ReleaseKey(Key::KeyE),
            TestEvent::PressKey(Key::KeyT),
            TestEvent::ReleaseKey(Key::KeyT),
            TestEvent::PressKey(Key::Enter),
            TestEvent::ExpectMode(Mode::WordPicking),
            // `A` is alpha's label, but the search still filters alpha out, so
            // this key matches nothing and word picking is not left.
            TestEvent::PressKey(Key::KeyA),
            TestEvent::ReleaseKey(Key::KeyA),
            TestEvent::ExpectMode(Mode::WordPicking),
            // The first backspace pops the hint key, the second drops the search
            // filter, so the same key now resolves to alpha.
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::PressKey(Key::KeyA),
            TestEvent::ReleaseKey(Key::KeyA),
            TestEvent::ExpectMode(Mode::TextActionMenu),
        ])
        .await;

        println!("Running Scenario 14: Backspace cancels the selected side of a range");
        run_test_scenario(vec![
            TestEvent::SetClipboard(FIVE_WORDS.to_string()),
            TestEvent::SendSignal(AppSignal::ReadClipboard),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::SendSignal(AppSignal::TextAction(TextAction::Split)),
            TestEvent::ExpectMode(Mode::WordPicking),
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
            // `A` anchors the range at alpha rather than leaving word picking:
            // the first pick of a range only marks one end.
            TestEvent::PressKey(Key::KeyA),
            TestEvent::ReleaseKey(Key::KeyA),
            TestEvent::ExpectMode(Mode::WordPicking),
            // Backspace drops that anchor ...
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            // ... so `B` anchors at beta instead of completing "alpha beta".
            TestEvent::PressKey(Key::KeyB),
            TestEvent::ReleaseKey(Key::KeyB),
            TestEvent::ExpectMode(Mode::WordPicking),
            // The re-anchor is real, not a stuck state: `C` now completes the
            // range "beta gamma" and lands on the text action menu.
            TestEvent::PressKey(Key::KeyC),
            TestEvent::ReleaseKey(Key::KeyC),
            TestEvent::ExpectMode(Mode::TextActionMenu),
        ])
        .await;

        println!("Running Scenario 15: Backspace in search pops the query, then leaves search");
        run_test_scenario(vec![
            TestEvent::SetClipboard(FIVE_WORDS.to_string()),
            TestEvent::SendSignal(AppSignal::ReadClipboard),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::SendSignal(AppSignal::TextAction(TextAction::Split)),
            TestEvent::ExpectMode(Mode::WordPicking),
            TestEvent::PressKey(Key::Slash),
            TestEvent::ReleaseKey(Key::Slash),
            TestEvent::ExpectMode(Mode::Searching(FilterMode::WordPicking)),
            TestEvent::PressKey(Key::KeyE),
            TestEvent::ReleaseKey(Key::KeyE),
            // One backspace only shortens the query, it does not leave search.
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::ExpectMode(Mode::Searching(FilterMode::WordPicking)),
            // With the query empty, backspace falls back to word picking.
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::ExpectMode(Mode::WordPicking),
        ])
        .await;

        println!("Running Scenario 16: An unmatched key is routed per menu kind");
        run_test_scenario(vec![
            // No selection behind the dashboard, so it must be refreshed through
            // its own signal — `MenuRefresh` is resolved from a selection and
            // would draw nothing, leaving the key without an answer.
            TestEvent::SetMode(Mode::DashBoard),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyX),
            TestEvent::ExpectSignal(AppSignal::DashboardRefresh("X".into())),
            TestEvent::ReleaseKey(Key::KeyX),
            // A menu that does have a selection keeps using `MenuRefresh`.
            TestEvent::SetMode(Mode::TextActionMenu),
            TestEvent::ClearSignals,
            TestEvent::PressKey(Key::KeyX),
            TestEvent::ExpectSignal(AppSignal::MenuRefresh("X".into())),
            TestEvent::ReleaseKey(Key::KeyX),
        ])
        .await;

        println!("Running Scenario 17: Backspace recovers the dashboard from an unmatched key");
        run_test_scenario(vec![
            TestEvent::SetMode(Mode::DashBoard),
            TestEvent::ClearSignals,
            // An unmatched key is answered by rebuilding the dashboard around it ...
            TestEvent::PressKey(Key::KeyX),
            TestEvent::ExpectSignal(AppSignal::DashboardRefresh("X".into())),
            TestEvent::ReleaseKey(Key::KeyX),
            TestEvent::ClearSignals,
            // ... and backspace pops that prefix, which is what the answer asks
            // the user to do.
            TestEvent::PressKey(Key::Backspace),
            TestEvent::ExpectSignal(AppSignal::DashboardRefresh("".into())),
            TestEvent::ReleaseKey(Key::Backspace),
            TestEvent::ClearSignals,
            // The menu is usable again: the next key is not swallowed by the typo.
            TestEvent::PressKey(Key::KeyC),
            TestEvent::ExpectSignal(AppSignal::ReadClipboard),
            TestEvent::ReleaseKey(Key::KeyC),
        ])
        .await;

        println!("Running Scenario 18: A tap on a clickable target is a sticky modifier");
        run_test_scenario(vec![
            // The default target is clickable, so a tap becomes a click modifier.
            TestEvent::SetMode(Mode::Filtering),
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
            TestEvent::ExpectMessage("Click modifiers: Shift".into()),
            // Each key toggles on its own, in a fixed written order ...
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Meta)),
            TestEvent::ExpectMessage("Click modifiers: Shift+Meta".into()),
            // ... and a second tap rolls that one back off.
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
            TestEvent::ExpectMessage("Click modifiers: Meta".into()),
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Meta)),
            TestEvent::ExpectMessage("Click modifiers: none".into()),
        ])
        .await;

        println!("Running Scenario 19: Text at hand takes the tap, whatever the key");
        run_test_scenario(vec![
            TestEvent::SetClipboard(FIVE_WORDS.to_string()),
            TestEvent::SendSignal(AppSignal::ReadClipboard),
            TestEvent::ExpectMode(Mode::TextActionMenu),
            TestEvent::SendSignal(AppSignal::TextAction(TextAction::Split)),
            TestEvent::ExpectMode(Mode::WordPicking),
            // The target is still the default clickable one, so this pins the
            // order: the open word picker wins over it.
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Ctrl)),
            TestEvent::ExpectMessage("Multi-selection is now on.".into()),
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Ctrl)),
            TestEvent::ExpectMessage("Multi-selection is now off.".into()),
        ])
        .await;

        println!("Running Scenario 20: A tap with nothing to carry is silent");
        run_test_scenario(vec![
            TestEvent::SendSignal(AppSignal::Activate(Target::Image)),
            TestEvent::SetMode(Mode::Filtering),
            TestEvent::ClearMessages,
            TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
            TestEvent::ExpectNoMessage,
        ])
        .await;
    });

    println!("All lifecycle integration tests passed!");
}

async fn run_test_scenario(events: Vec<TestEvent>) {
    run_test_scenario_with_config(events, GlyphlowConfig::default()).await;
}

async fn run_test_scenario_with_config(events: Vec<TestEvent>, config: GlyphlowConfig) {
    // Setup shared state
    let state = Arc::new(Mutex::new(Mode::Idle));
    let key_state = Arc::new(Mutex::new(KeyState::default()));
    let (tx, mut rx) = mpsc::channel::<AppSignal>(100);
    let done = Arc::new(AtomicBool::new(false));
    let processed_signals = Arc::new(Mutex::new(Vec::new()));

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
    let sim_tx = tx.clone();

    let sim_thread = std::thread::spawn(move || {
        let wait_timeout = Duration::from_millis(1500);

        for (idx, event) in events.into_iter().enumerate() {
            let step = idx + 1;
            match event {
                TestEvent::PressKey(key) => {
                    sim_key_state.lock().unwrap().key_down(&key);
                    key_listener.key_down(key, &sim_state, &mut sim_key_state.lock().unwrap());
                }
                TestEvent::ReleaseKey(key) => {
                    sim_key_state.lock().unwrap().key_up(&key);
                }
                TestEvent::SetMode(mode) => {
                    *sim_state.lock().unwrap() = mode;
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
                        "step {step}: expected mode {expected_mode:?}, but got {current_mode:?}"
                    );
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
                        "step {step}: expected signal {expected_signal:?} was not processed. \
                         Processed signals: {:?}",
                        *sim_processed_signals.lock().unwrap()
                    );
                }
                TestEvent::ClearSignals => {
                    sim_processed_signals.lock().unwrap().clear();
                }
                TestEvent::ClearMessages => {
                    MESSAGES.lock().unwrap().clear();
                }
                TestEvent::ExpectMessage(expected) => {
                    let start = std::time::Instant::now();
                    let mut found = false;
                    while start.elapsed() < wait_timeout {
                        if MESSAGES.lock().unwrap().contains(&expected) {
                            found = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    assert!(
                        found,
                        "step {step}: expected notification {expected:?}. Captured: {:?}",
                        *MESSAGES.lock().unwrap()
                    );
                }
                TestEvent::ExpectNoMessage => {
                    std::thread::sleep(wait_timeout / 5);
                    let messages = MESSAGES.lock().unwrap();
                    assert!(
                        messages.is_empty(),
                        "step {step}: expected no notification, got {messages:?}"
                    );
                }
                TestEvent::SetClipboard(text) => {
                    text_to_clipboard(&text);
                }
                TestEvent::SendSignal(signal) => {
                    sim_tx
                        .blocking_send(signal)
                        .expect("Failed to send signal to the engine");
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
            processed_signals.lock().unwrap().push(signal.clone());
            app_engine.handle_signal(signal).await;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    sim_thread.join().unwrap();
    let _ = std::fs::remove_file(cache_file);
}
