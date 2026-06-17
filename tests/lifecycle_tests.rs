use glyphlow::{AppEngine, AppSignal, KeyListener, KeyState, Mode, config::GlyphlowConfig};
use monio::Key;
use objc2::MainThreadMarker;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc;

fn main() {
    let _mtm = MainThreadMarker::new().expect("This test must run on the main thread");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        test_app_lifecycle_keystrokes().await;
    });

    println!("All lifecycle integration tests passed!");
}

async fn test_app_lifecycle_keystrokes() {
    // Setup shared state
    let state = Arc::new(Mutex::new(Mode::Idle));
    let key_state = Arc::new(Mutex::new(KeyState::default()));
    let (tx, mut rx) = mpsc::channel::<AppSignal>(100);
    let done = Arc::new(AtomicBool::new(false));

    // Load default config
    let config = GlyphlowConfig::default();

    // Create a temporary cache file
    let temp_dir = std::env::temp_dir();
    let cache_file = temp_dir.join("glyphlow_test_tempfile.md");
    if !cache_file.exists() {
        std::fs::File::create(&cache_file).unwrap();
    }

    // Create KeyListener (takes config by ref) first, then AppEngine (takes config by value)
    let key_listener = KeyListener::new(tx.clone(), &config);

    let mut app_engine = AppEngine::new(
        state.clone(),
        key_state.clone(),
        config,
        cache_file.clone(),
        tx.clone(),
    );

    // Spawn background thread for keystroke simulation
    let sim_state = state.clone();
    let sim_key_state = key_state.clone();
    let sim_done = done.clone();

    let sim_thread = std::thread::spawn(move || {
        let wait_timeout = Duration::from_secs(2);

        // Assert initial state is Idle
        assert_eq!(*sim_state.lock().unwrap(), Mode::Idle);

        // --- Step 1: Simulate the activation hotkey combination (AltLeft + G) ---
        println!("[Sim] Pressing AltLeft + G...");
        {
            let mut ks = sim_key_state.lock().unwrap();
            ks.key_down(&Key::AltLeft);
        }
        let swallowed =
            key_listener.key_down(Key::KeyG, &sim_state, &mut sim_key_state.lock().unwrap());
        assert!(
            swallowed,
            "The global activation hotkey should be swallowed"
        );

        // Wait for state to transition to DashBoard
        wait_for_states(&sim_state, &[Mode::DashBoard], wait_timeout);
        println!("[Sim] State successfully changed to DashBoard!");

        // --- Step 2: In DashBoard mode, press "T" (Key::KeyT) to activate Text target ---
        // Reset key_state first
        {
            let mut ks = sim_key_state.lock().unwrap();
            ks.clear_prefix();
            ks.key_up(&Key::AltLeft);
            ks.key_up(&Key::KeyG);
        }

        // Wait a tiny bit for the main thread to finish processing the MenuRefresh signal
        std::thread::sleep(Duration::from_millis(50));

        println!("[Sim] Pressing T to activate Text target...");
        let swallowed =
            key_listener.key_down(Key::KeyT, &sim_state, &mut sim_key_state.lock().unwrap());
        assert!(swallowed, "The menu key 'T' should be swallowed");

        // Wait for state to transition to Filtering or WaitAndDeactivate
        let next_mode = wait_for_states(
            &sim_state,
            &[Mode::Filtering, Mode::WaitAndDeactivate],
            wait_timeout,
        );
        println!("[Sim] State transitioned to {:?}", next_mode);

        // --- Step 3: Test pressing Space key to Deactivate back to Idle ---
        {
            let mut ks = sim_key_state.lock().unwrap();
            ks.clear_prefix();
        }

        // Wait a tiny bit for the main thread to handle the activate signal
        std::thread::sleep(Duration::from_millis(50));

        println!("[Sim] Pressing Space/Escape to deactivate...");
        let swallowed =
            key_listener.key_down(Key::Escape, &sim_state, &mut sim_key_state.lock().unwrap());
        assert!(swallowed, "Deactivation key should be swallowed");

        // Wait for state to return to Idle
        wait_for_states(&sim_state, &[Mode::Idle], wait_timeout);
        println!("[Sim] State successfully returned to Idle!");

        sim_done.store(true, Ordering::Relaxed);
    });

    // Main thread event loop
    let loop_timeout = Duration::from_secs(5);
    let start_time = std::time::Instant::now();
    while !done.load(Ordering::Relaxed) {
        if start_time.elapsed() > loop_timeout {
            panic!("Test timed out in main event loop");
        }

        if let Ok(signal) = rx.try_recv() {
            println!("[Main] Processing signal: {:?}", signal);
            app_engine.handle_signal(signal).await;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    sim_thread.join().unwrap();
    let _ = std::fs::remove_file(cache_file);
}

fn wait_for_states(state: &Arc<Mutex<Mode>>, targets: &[Mode], timeout: Duration) -> Mode {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let current = state.lock().unwrap().clone();
        if targets.contains(&current) {
            return current;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Timeout waiting for states {:?}", targets);
}
