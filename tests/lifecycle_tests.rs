use glyphlow::{
    AppEngine, AppSignal, FilterMode, KeyListener, KeyState, Mode, ModifierKey, ScrollAction,
    TextAction,
    action::text_to_clipboard,
    ax_element::Target,
    config::{GlyphlowConfig, RoleOfInterest, WorkFlow},
};
use monio::Key;
use objc2::MainThreadMarker;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::mpsc;

/// Five words, so the default alphabet gives every word a single-key label:
/// `alpha` = `A`, `beta` = `B`, `gamma` = `C`, `delta` = `D`, `zeta` = `E`.
const FIVE_WORDS: &str = "alpha beta gamma delta zeta";

/// Stands for "no scenario is being serviced", so a notification logged outside
/// the event loop has no slot to land in.
const NO_SCENARIO: usize = usize::MAX;

/// The scenario whose engine the driver is servicing, or [`NO_SCENARIO`].
///
/// Only the driver writes it, and the engine only logs from the main thread —
/// the thread the driver runs on — so a record always belongs to whichever
/// scenario is mid-`handle_signal`.
static CURRENT_SCENARIO: AtomicUsize = AtomicUsize::new(NO_SCENARIO);

/// Notifications the engines showed, one slot per scenario, in order.
///
/// What a modifier tap did is not visible on the signal channel — it shows up
/// only as a notification — so the log is where a test can read it back.
static MESSAGES: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

/// The clipboard is machine-wide, so the scenarios that use it take turns.
static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

struct MessageLog;

impl log::Log for MessageLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let current = CURRENT_SCENARIO.load(Ordering::Relaxed);
        if let Ok(mut messages) = MESSAGES.lock()
            && let Some(slot) = messages.get_mut(current)
        {
            slot.push(record.args().to_string());
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
    /// Waits as long, and fails if the engine was given anything at all.
    ExpectNoSignal,
    ClearSignals,
    SetClipboard(String),
    /// Sends a raw signal, the way the CLI would over its socket, and waits for
    /// the engine to finish with it.
    SendSignal(AppSignal),
    /// Drops the notifications captured so far.
    ClearMessages,
    /// Waits for a notification whose text is exactly this.
    ExpectMessage(String),
    /// Waits as long, and fails if anything was notified at all.
    ExpectNoMessage,
}

/// A scenario's fixed inputs.
struct Scenario {
    name: &'static str,
    events: Vec<TestEvent>,
    config: GlyphlowConfig,
}

impl Scenario {
    /// A scenario built on the default config.
    fn new(name: &'static str, events: Vec<TestEvent>) -> Self {
        Self {
            name,
            events,
            config: GlyphlowConfig::default(),
        }
    }

    /// Whether the scenario reads or writes the system clipboard.
    ///
    /// A key press can reach it too — `C` on the dashboard reads it — but the
    /// scenarios that do that set it as well, which is what this looks for.
    fn uses_clipboard(&self) -> bool {
        self.events.iter().any(|event| {
            matches!(
                event,
                TestEvent::SetClipboard(_) | TestEvent::SendSignal(AppSignal::ReadClipboard)
            )
        })
    }
}

fn main() {
    let _mtm = MainThreadMarker::new().expect("This test must run on the main thread");

    log::set_logger(&MESSAGE_LOG).expect("the test binary owns the logger");
    log::set_max_level(log::LevelFilter::Info);

    let scenarios = vec![
        Scenario::new(
            "Scenario 1: Idle -> Dashboard -> Idle (Deactivation)",
            vec![
                TestEvent::ExpectMode(Mode::Idle),
                TestEvent::PressKey(Key::AltLeft),
                TestEvent::PressKey(Key::KeyG),
                TestEvent::ReleaseKey(Key::AltLeft),
                TestEvent::ReleaseKey(Key::KeyG),
                TestEvent::ExpectMode(Mode::DashBoard),
                TestEvent::PressKey(Key::Escape),
                TestEvent::ReleaseKey(Key::Escape),
                TestEvent::ExpectMode(Mode::Idle),
            ],
        ),
        Scenario::new(
            "Scenario 2: Filtering Mode Keys",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 3: Searching Mode Keys",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 4: Scrolling Mode Keys",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 5: Text Action Menu Mode & Copy Action",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 6: Word Picking Mode & Searching inside Word Picking",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 7: Image Action Menu Mode",
            vec![
                TestEvent::SetMode(Mode::ImageActionMenu),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::KeyO), // Image OCR action
                TestEvent::ExpectSignal(AppSignal::FrameOCR),
                TestEvent::ReleaseKey(Key::KeyO),
            ],
        ),
        Scenario::new(
            "Scenario 8: Dictionary Scrolling Mode",
            vec![
                TestEvent::SetMode(Mode::DictionaryScrolling),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::Backspace), // Backspace when prefix is empty -> Back to TextActionMenu
                TestEvent::ExpectSignal(AppSignal::BackToTextActionMenu),
                TestEvent::ReleaseKey(Key::Backspace),
            ],
        ),
        Scenario::new(
            "Scenario 9: OCR Result Filtering Mode",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 10: Wait and Deactivate Mode",
            vec![
                TestEvent::SetMode(Mode::WaitAndDeactivate),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::KeyA), // Any key should deactivate
                TestEvent::ExpectMode(Mode::Idle),
                TestEvent::ExpectSignal(AppSignal::DeActivate),
                TestEvent::ReleaseKey(Key::KeyA),
            ],
        ),
        Scenario {
            name: "Scenario 11: CLI driven workflow by name",
            // These bypass the server's focused-window pre-selection (the
            // harness has no accessibility tree), so only paths that do not
            // depend on the current selection are asserted here. The role-based
            // "offer elements for picking" path needs a live accessibility tree
            // and is not covered.
            config: {
                let mut config = GlyphlowConfig::default();
                config.workflows.push(WorkFlow {
                    display: "Other App Only".into(),
                    key: "Z".into(),
                    valid_app_ids: Some(vec!["com.example.not-the-focused-app".into()]),
                    starting_role: RoleOfInterest::Any,
                    actions: vec![],
                });
                config
            },
            events: vec![
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
        },
        Scenario::new(
            "Scenario 12: Backspace leaves word picking for the text action menu",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 13: Backspace pops the hint key, then the search filter",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 14: Backspace cancels the selected side of a range",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 15: Backspace in search pops the query, then leaves search",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 16a: An unmatched key is routed per menu kind",
            // No selection behind the dashboard, so it must be refreshed through
            // its own signal — `MenuRefresh` is resolved from a selection and
            // would draw nothing, leaving the key without an answer.
            vec![
                TestEvent::SetMode(Mode::DashBoard),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::KeyX),
                TestEvent::ExpectSignal(AppSignal::DashboardRefresh("X".into())),
                TestEvent::ReleaseKey(Key::KeyX),
            ],
        ),
        Scenario::new(
            "Scenario 16b: A menu with a selection keeps using MenuRefresh",
            // Its own scenario, because an unmatched key stays in
            // `KeyState::prefix`: sharing one would prepend the dashboard's `X`
            // to this one's.
            vec![
                TestEvent::SetMode(Mode::TextActionMenu),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::KeyX),
                TestEvent::ExpectSignal(AppSignal::MenuRefresh("X".into())),
                TestEvent::ReleaseKey(Key::KeyX),
            ],
        ),
        Scenario::new(
            "Scenario 17: Backspace recovers the dashboard from an unmatched key",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 18: A tap on a clickable target is a sticky modifier",
            vec![
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
                TestEvent::ExpectMessage("Click modifiers: None".into()),
            ],
        ),
        Scenario::new(
            "Scenario 19: Text at hand takes the tap, whatever the key",
            vec![
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
            ],
        ),
        Scenario::new(
            "Scenario 20: A tap with nothing to carry is silent",
            vec![
                TestEvent::SendSignal(AppSignal::Activate(Target::Image)),
                TestEvent::SetMode(Mode::Filtering),
                TestEvent::ClearMessages,
                TestEvent::SendSignal(AppSignal::ToggleModifier(ModifierKey::Shift)),
                TestEvent::ExpectNoMessage,
            ],
        ),
        Scenario::new(
            "Scenario 21: Grid Mode Keys",
            vec![
                TestEvent::SetMode(Mode::Grid),
                TestEvent::ClearSignals,
                // A cell key reaches the engine as the character it types ...
                TestEvent::PressKey(Key::KeyK),
                TestEvent::ExpectSignal(AppSignal::GridKey('K')),
                TestEvent::ReleaseKey(Key::KeyK),
                TestEvent::ClearSignals,
                // ... enter keeps the cell and runs the rest of the workflow ...
                TestEvent::PressKey(Key::Enter),
                TestEvent::ExpectSignal(AppSignal::GridAccept),
                TestEvent::ReleaseKey(Key::Enter),
                TestEvent::ClearSignals,
                // ... and either delete key widens by one level.
                TestEvent::PressKey(Key::Backspace),
                TestEvent::ExpectSignal(AppSignal::GridBack),
                TestEvent::ReleaseKey(Key::Backspace),
                TestEvent::ClearSignals,
                TestEvent::PressKey(Key::Delete),
                TestEvent::ExpectSignal(AppSignal::GridBack),
                TestEvent::ReleaseKey(Key::Delete),
                TestEvent::ClearSignals,
                // A space is ignored rather than closing the grid, unlike every menu
                // mode — here that would throw the run away on a mistyped cell key.
                TestEvent::PressKey(Key::Space),
                TestEvent::ExpectNoSignal,
                TestEvent::ReleaseKey(Key::Space),
                TestEvent::ClearSignals,
                // Esc abandons the run, so the mode comes back to Idle with it.
                TestEvent::PressKey(Key::Escape),
                TestEvent::ExpectSignal(AppSignal::DeActivate),
                TestEvent::ExpectMode(Mode::Idle),
                TestEvent::ReleaseKey(Key::Escape),
            ],
        ),
    ];

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(run_scenarios(scenarios));

    println!("All lifecycle integration tests passed!");
}

/// One live scenario: its engine, the signals waiting to be handled, and the
/// simulator thread feeding them in.
struct Run {
    name: &'static str,
    /// This scenario's slot in [`MESSAGES`].
    slot: usize,
    engine: AppEngine,
    rx: mpsc::Receiver<AppSignal>,
    /// The signals the engine has finished handling, in order.
    processed: Arc<Mutex<Vec<AppSignal>>>,
    done: Arc<AtomicBool>,
    sim: JoinHandle<()>,
    cache_file: PathBuf,
}

impl Run {
    /// Build the engine — which needs the main thread — and start replaying the
    /// scenario's events into it.
    fn start(slot: usize, scenario: Scenario) -> Self {
        println!("Running {}", scenario.name);

        let state = Arc::new(Mutex::new(Mode::Idle));
        let key_state = Arc::new(Mutex::new(KeyState::default()));
        let (tx, rx) = mpsc::channel::<AppSignal>(100);
        let done = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(Mutex::new(Vec::new()));

        // A path of its own, so scenarios running together cannot read or delete
        // each other's editor file.
        let cache_file = std::env::temp_dir().join(format!("glyphlow_test_tempfile_{slot}.md"));
        if !cache_file.exists() {
            std::fs::File::create(&cache_file).unwrap();
        }

        let uses_clipboard = scenario.uses_clipboard();
        let key_listener = KeyListener::new(tx.clone(), &scenario.config);
        let engine = AppEngine::new(
            state.clone(),
            key_state.clone(),
            scenario.config,
            cache_file.clone(),
            tx.clone(),
        );

        let sim_state = state.clone();
        let sim_key_state = key_state.clone();
        let sim_processed = processed.clone();
        let sim_done = done.clone();
        let sim_tx = tx.clone();
        let events = scenario.events;

        let sim = std::thread::spawn(move || {
            // Held for the whole scenario, so a set-then-read pair of clipboard
            // operations cannot be split by another scenario's write.
            let _clipboard = uses_clipboard.then(|| CLIPBOARD_LOCK.lock().unwrap());

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
                            if sim_processed.lock().unwrap().contains(&expected_signal) {
                                found = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        assert!(
                            found,
                            "step {step}: expected signal {expected_signal:?} was not processed. \
                             Processed signals: {:?}",
                            *sim_processed.lock().unwrap()
                        );
                    }
                    TestEvent::ExpectNoSignal => {
                        std::thread::sleep(wait_timeout / 5);
                        let signals = sim_processed.lock().unwrap();
                        assert!(
                            signals.is_empty(),
                            "step {step}: expected no signal, got {signals:?}"
                        );
                    }
                    TestEvent::ClearSignals => {
                        sim_processed.lock().unwrap().clear();
                    }
                    TestEvent::ClearMessages => {
                        MESSAGES.lock().unwrap()[slot].clear();
                    }
                    TestEvent::ExpectMessage(expected) => {
                        let start = std::time::Instant::now();
                        let mut found = false;
                        while start.elapsed() < wait_timeout {
                            if MESSAGES.lock().unwrap()[slot].contains(&expected) {
                                found = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        assert!(
                            found,
                            "step {step}: expected notification {expected:?}. Captured: {:?}",
                            MESSAGES.lock().unwrap()[slot]
                        );
                    }
                    TestEvent::ExpectNoMessage => {
                        std::thread::sleep(wait_timeout / 5);
                        let messages = MESSAGES.lock().unwrap();
                        assert!(
                            messages[slot].is_empty(),
                            "step {step}: expected no notification, got {:?}",
                            messages[slot]
                        );
                    }
                    TestEvent::SetClipboard(text) => {
                        text_to_clipboard(&text);
                    }
                    TestEvent::SendSignal(signal) => {
                        let handled = || {
                            sim_processed
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|seen| **seen == signal)
                                .count()
                        };
                        let before = handled();
                        sim_tx
                            .blocking_send(signal.clone())
                            .expect("Failed to send signal to the engine");
                        // Wait for the engine to finish with it, so anything it
                        // produces — a notification, say — is already in place
                        // before the next event can clear it away.
                        let start = std::time::Instant::now();
                        while handled() <= before {
                            assert!(
                                start.elapsed() < wait_timeout,
                                "step {step}: signal {signal:?} was never handled"
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                // Add a small delay between events to ensure orderly processing
                std::thread::sleep(Duration::from_millis(50));
            }

            sim_done.store(true, Ordering::Relaxed);
        });

        Self {
            name: scenario.name,
            slot,
            engine,
            rx,
            processed,
            done,
            sim,
            cache_file,
        }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    fn finish(self) {
        self.sim.join().unwrap();
        let _ = std::fs::remove_file(&self.cache_file);
    }
}

/// Run every scenario at once.
///
/// The engines all live on the main thread — `AppEngine::new` needs the marker
/// and the drawing layer needs the main thread — so "in parallel" means the
/// simulator threads really do run concurrently while the driver services
/// whichever engine has a signal waiting. The scenarios share only the system
/// clipboard, which the ones that use it take turns on.
async fn run_scenarios(scenarios: Vec<Scenario>) {
    MESSAGES
        .lock()
        .unwrap()
        .extend(scenarios.iter().map(|_| Vec::new()));

    let mut runs: Vec<Run> = scenarios
        .into_iter()
        .enumerate()
        .map(|(slot, scenario)| Run::start(slot, scenario))
        .collect();

    let loop_timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    while runs.iter().any(|run| !run.is_done()) {
        // A simulator that panicked never reaches its `done` flag; report the
        // scenario rather than spinning until the loop times out.
        if let Some(dead) = runs
            .iter()
            .find(|run| !run.is_done() && run.sim.is_finished())
        {
            panic!("Scenario `{}` stopped before it finished", dead.name);
        }
        if start.elapsed() > loop_timeout {
            let stuck: Vec<_> = runs
                .iter()
                .filter(|run| !run.is_done())
                .map(|run| run.name)
                .collect();
            panic!("Test timed out in the main event loop; still running: {stuck:?}");
        }

        for run in &mut runs {
            while let Ok(signal) = run.rx.try_recv() {
                CURRENT_SCENARIO.store(run.slot, Ordering::Relaxed);
                run.engine.handle_signal(signal.clone()).await;
                run.processed.lock().unwrap().push(signal);
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    for run in runs {
        run.finish();
    }
}
