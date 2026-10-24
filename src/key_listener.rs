use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use monio::Key;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::{
    app_engine::MenuScope,
    ax_element::Target,
    config::{AlphabeticKey, GlyphlowConfig, KeyBinding},
    util::SEARCH_TERM_SEPARATOR,
};

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum TextAction {
    Copy,
    Dictionary,
    Split,
    Editor,
    /// index of the action in the config
    UserDefined(usize),
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum ScrollAction {
    UpLeft,
    DownRight,
    IncreaseDistance,
    DecreaseDistance,
    Top,
    Bottom,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum FilterMode {
    WordPicking,
    Generic,
    OCR,
}

impl FilterMode {
    pub fn to_app_mode(&self) -> Mode {
        match self {
            FilterMode::WordPicking => Mode::WordPicking,
            FilterMode::Generic => Mode::Filtering,
            FilterMode::OCR => Mode::OCRResultFiltering,
        }
    }
}

/// A modifier key Glyphlow can hold down on the user's behalf.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum ModifierKey {
    Shift,
    Ctrl,
    Alt,
    Meta,
    Tab,
}

impl ModifierKey {
    /// Which modifier a physical key stands for, if it is one at all.
    fn of(key: &Key) -> Option<Self> {
        match key {
            Key::ShiftLeft | Key::ShiftRight => Some(Self::Shift),
            Key::ControlLeft | Key::ControlRight => Some(Self::Ctrl),
            Key::AltLeft | Key::AltRight => Some(Self::Alt),
            Key::MetaLeft | Key::MetaRight => Some(Self::Meta),
            Key::Tab => Some(Self::Tab),
            _ => None,
        }
    }

    /// The key to hold down to produce it.
    pub fn key(self) -> Key {
        match self {
            Self::Shift => Key::ShiftLeft,
            Self::Ctrl => Key::ControlLeft,
            Self::Alt => Key::AltLeft,
            Self::Meta => Key::MetaLeft,
            Self::Tab => Key::Tab,
        }
    }

    /// The name this modifier is written as.
    pub fn label(self) -> &'static str {
        match self {
            Self::Shift => "Shift",
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Meta => "Meta",
            Self::Tab => "Tab",
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum AppSignal {
    // State signals
    Activate(Target),
    DeActivate,
    /// A key was typed while filtering hints.
    HintFilter(char, FilterMode),
    /// A key was typed while searching. A space arrives as
    /// [`crate::util::SEARCH_TERM_SEPARATOR`], not as `' '`.
    SearchFilter(char, FilterMode),
    /// A key was typed while the recursive grid was up.
    GridKey(char),
    /// Enter while the recursive grid was up: keep the cell and carry on.
    GridAccept,
    /// Backspace while the recursive grid was up: widen to the level above.
    GridBack,
    /// The global key was pressed: put the dashboard up.
    OpenDashboard,
    /// A character typed while a menu is up. Which row it completes depends on
    /// the selection, so the engine resolves it.
    MenuKey(char),
    /// Backspace or Delete while a menu is up.
    MenuBackspace,
    ActOnEnter,
    // Sub state signals
    FileUpdate(PathBuf),
    ClearNotification(usize),
    /// A modifier key was tapped while filtering hints.
    ToggleModifier(ModifierKey),
    // Menu specific
    TextAction(TextAction),
    ScrollAction(ScrollAction),
    // Generic Actions
    RunWorkFlow(usize),
    /// Run a workflow addressed by (a fragment of) its display name,
    /// resolved against the server side config. Used by the CLI.
    RunWorkFlowByName(String),
    FrameOCR,
    StartSearch,
    FinishSearch(FilterMode),
    /// A debounced search re-check. The id is the counter it was scheduled with,
    /// so a timer that has been superseded can be ignored.
    SearchDebounce(usize, FilterMode),
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Mode {
    Filtering,
    Idle,
    /// A keyed menu is up, and `MenuScope` says which rows it offers.
    Menu(MenuScope),
    /// A dictionary is on screen. Not a menu: it is scrolled by the keys
    /// [`MenuScope::Scroll`] resolves.
    DictionaryScrolling,
    WordPicking,
    /// A search is open, started from the given filtering mode.
    Searching(FilterMode),
    OCRResultFiltering,
    /// The recursive grid is up, waiting for a cell key, Enter, Backspace or Esc.
    Grid,
    /// A notification is on screen; any key dismisses everything.
    WaitAndDeactivate,
}

/// Turns keys into signals.
///
/// Nothing here knows about menus: a menu key is forwarded unresolved, because
/// which row it completes depends on the selection, which only the engine has.
#[derive(Debug)]
pub struct KeyListener {
    sender: Sender<AppSignal>,
    global_key_binding: KeyBinding,
}

impl KeyListener {
    pub fn new(sender: Sender<AppSignal>, config: &GlyphlowConfig) -> KeyListener {
        KeyListener {
            sender,
            global_key_binding: config.global_trigger_key.clone(),
        }
    }

    fn send(&self, signal: AppSignal) {
        if let Err(e) = self.sender.blocking_send(signal) {
            log::error!("Error sending signal: {}", e);
        }
    }

    fn handle_menu_key(&self, key: &Key, state: &mut Mode) -> bool {
        match key {
            Key::Escape | Key::Space => {
                self.send(AppSignal::DeActivate);
                *state = Mode::Idle;
            }
            Key::Backspace | Key::Delete => self.send(AppSignal::MenuBackspace),
            _ => {
                // Keys that type nothing cannot complete a row. A bare Shift is
                // not one of them: it is the `󰘶` in `󰘶G`.
                let key_char = key.to_char();
                if key_char != ' ' {
                    self.send(AppSignal::MenuKey(key_char));
                }
            }
        }
        true
    }

    fn handle_filter_key(
        &self,
        key: &Key,
        mut state: MutexGuard<'_, Mode>,
        mode: FilterMode,
    ) -> bool {
        // Modifiers never type anything, so they are claimed before the
        // catch-all below can read them as a space and deactivate.
        if let Some(modifier) = ModifierKey::of(key) {
            self.send(AppSignal::ToggleModifier(modifier));
            return true;
        }

        match key {
            Key::Slash => {
                self.send(AppSignal::StartSearch);
                *state = Mode::Searching(mode)
            }
            Key::Enter => self.send(AppSignal::ActOnEnter),
            _ => {
                let key_char = key.to_char();
                if key_char == ' ' {
                    self.send(AppSignal::DeActivate);
                    *state = Mode::Idle;
                } else {
                    self.send(AppSignal::HintFilter(key_char, mode));
                }
            }
        }
        true
    }

    /// Returns true if key is effective, and should be swallowed by this app
    pub fn key_down(&self, key: Key, state: &Mutex<Mode>, key_state: &KeyState) -> bool {
        let Ok(mut state) = state.try_lock() else {
            return false;
        };

        match *state {
            Mode::Idle => {
                if self.global_key_binding.keys.iter().all(|k| {
                    k == &key
                        || key_state.pressed_keys.contains(k)
                        || k.right_alternative()
                            .is_some_and(|r| *k == r || key_state.pressed_keys.contains(&r))
                }) {
                    self.send(AppSignal::OpenDashboard);
                    // `draw_dashboard` sets this on the signal, but the keys
                    // that arrive before the engine handles it have to be eaten
                    // as menu keys already, so the scope is claimed here too.
                    *state = Mode::Menu(MenuScope::Dashboard);
                    true
                } else {
                    false
                }
            }
            Mode::Menu(_) | Mode::DictionaryScrolling => self.handle_menu_key(&key, &mut state),
            Mode::WordPicking => self.handle_filter_key(&key, state, FilterMode::WordPicking),
            Mode::Filtering => self.handle_filter_key(&key, state, FilterMode::Generic),
            Mode::OCRResultFiltering => self.handle_filter_key(&key, state, FilterMode::OCR),
            Mode::Searching(mode) => {
                match key {
                    Key::Enter => self.send(AppSignal::FinishSearch(mode)),
                    Key::ShiftLeft | Key::ShiftRight => (),
                    Key::Escape => {
                        self.send(AppSignal::DeActivate);
                        *state = Mode::Idle
                    }
                    _ => {
                        let key_char = if key_state.pressed_keys.contains(&Key::ShiftLeft)
                            || key_state.pressed_keys.contains(&Key::ShiftRight)
                        {
                            key.shifted_char()
                        } else {
                            key.to_char()
                        };
                        let key_char = if key_char == ' ' {
                            SEARCH_TERM_SEPARATOR
                        } else {
                            key_char
                        };
                        self.send(AppSignal::SearchFilter(key_char, mode));
                    }
                };
                true
            }
            Mode::Grid => {
                if let Some(modifier) = ModifierKey::of(&key) {
                    self.send(AppSignal::ToggleModifier(modifier));
                    return true;
                }
                match key {
                    Key::Enter => self.send(AppSignal::GridAccept),
                    Key::Backspace | Key::Delete => self.send(AppSignal::GridBack),
                    Key::Escape => {
                        self.send(AppSignal::DeActivate);
                        *state = Mode::Idle;
                    }
                    _ => {
                        // Keys that type nothing cannot label a cell, so they
                        // are ignored rather than closing the grid.
                        let key_char = key.to_char();
                        if key_char != ' ' {
                            self.send(AppSignal::GridKey(key_char));
                        }
                    }
                }
                true
            }
            Mode::WaitAndDeactivate => {
                self.send(AppSignal::DeActivate);
                *state = Mode::Idle;
                true
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct KeyState {
    pub pressed_keys: HashSet<Key>,
    /// Set while Glyphlow is synthesizing keys itself, so the event tap does not
    /// feed its own events back in.
    pub is_simulating: bool,
}

impl KeyState {
    pub fn key_down(&mut self, key: &Key) {
        self.pressed_keys.insert(*key);
    }

    pub fn key_up(&mut self, key: &Key) {
        self.pressed_keys.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI and the server talk JSON over a Unix socket, so the encoding of
    /// every signal the CLI can emit is part of the protocol. Pin it down so a
    /// future rename cannot silently break the client.
    #[test]
    fn test_cli_signal_wire_format() {
        let cases = [
            (
                AppSignal::Activate(Target::Clickable),
                r#"{"Activate":"Clickable"}"#,
            ),
            (AppSignal::Activate(Target::Text), r#"{"Activate":"Text"}"#),
            (
                AppSignal::Activate(Target::Image),
                r#"{"Activate":"Image"}"#,
            ),
            (
                AppSignal::Activate(Target::Editable),
                r#"{"Activate":"Editable"}"#,
            ),
            (AppSignal::Activate(Target::Edit), r#"{"Activate":"Edit"}"#),
            (
                AppSignal::Activate(Target::ChildElement),
                r#"{"Activate":"ChildElement"}"#,
            ),
            (
                AppSignal::Activate(Target::Scrollable),
                r#"{"Activate":"Scrollable"}"#,
            ),
            (
                AppSignal::RunWorkFlowByName("ProofRead".into()),
                r#"{"RunWorkFlowByName":"ProofRead"}"#,
            ),
        ];

        for (signal, expected) in cases {
            let encoded = serde_json::to_string(&signal).expect("signal should serialize");
            assert_eq!(encoded, expected);
            let decoded: AppSignal =
                serde_json::from_str(&encoded).expect("signal should deserialize");
            assert_eq!(decoded, signal);
        }
    }
}
