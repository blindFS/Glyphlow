use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use monio::Key;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::{
    ax_element::Target,
    config::{AlphabeticKey, GlyphlowConfig, KeyBinding, MenuKind, WorkFlow, WorkFlowAction},
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

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
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
}

impl ModifierKey {
    /// Which modifier a physical key stands for, if it is one at all.
    fn of(key: &Key) -> Option<Self> {
        match key {
            Key::ShiftLeft | Key::ShiftRight => Some(Self::Shift),
            Key::ControlLeft | Key::ControlRight => Some(Self::Ctrl),
            Key::AltLeft | Key::AltRight => Some(Self::Alt),
            Key::MetaLeft | Key::MetaRight => Some(Self::Meta),
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
    MenuRefresh(String),
    DashboardRefresh(String),
    ActOnEnter,
    // Sub state signals
    FileUpdate(PathBuf),
    ClearNotification(usize),
    /// A modifier key was tapped while filtering hints.
    ToggleModifier(ModifierKey),
    // Generic Actions
    RunWorkFlow(usize),
    /// Run a workflow addressed by (a fragment of) its display name,
    /// resolved against the server side config. Used by the CLI.
    RunWorkFlowByName(String),
    /// Run a chain of actions, the way a menu row does.
    ///
    /// Unlike [`RunWorkFlow`](Self::RunWorkFlow) the chain is not checked
    /// against the selection, and running out of actions leaves the menu up
    /// instead of ending the interaction.
    RunActions(Box<[WorkFlowAction]>),
    StartSearch,
    FinishSearch(FilterMode),
    /// A debounced search re-check. The id is the counter it was scheduled with,
    /// so a timer that has been superseded can be ignored.
    SearchDebounce(usize, FilterMode),
    BackToTextActionMenu,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Mode {
    DashBoard,
    Filtering,
    Idle,
    Scrolling,
    DictionaryScrolling,
    TextActionMenu,
    ImageActionMenu,
    WordPicking,
    /// A search is open, started from the given filtering mode.
    Searching(FilterMode),
    OCRResultFiltering,
    /// The recursive grid is up, waiting for a cell key, Enter, Backspace or Esc.
    Grid,
    /// A notification is on screen; any key dismisses everything.
    WaitAndDeactivate,
}

#[derive(Debug)]
pub struct KeyListener {
    menu_actions: HashMap<MenuKind, HashMap<String, AppSignal>>,
    menu_action_max_key_len: HashMap<MenuKind, usize>,
    sender: Sender<AppSignal>,
    global_key_binding: KeyBinding,
}

impl KeyListener {
    /// The signal that runs a menu row's chain.
    fn run_chain(actions: impl Into<Box<[WorkFlowAction]>>) -> AppSignal {
        AppSignal::RunActions(actions.into())
    }

    fn build_menu_actions(
        menu: MenuKind,
        config: &GlyphlowConfig,
    ) -> (HashMap<String, AppSignal>, usize) {
        let mut items = HashMap::new();

        // Order matters: the first entry to claim a key keeps it, and this is the
        // order the menu draws its rows in — rows bound to this menu, the editor,
        // the text actions, then the workflows offered wherever their role allows.
        Self::add_workflows(&mut items, menu.rows(config));

        if let Some(chain) = menu.editor_row()
            && let Some(editor) = config.editor.as_ref()
        {
            items
                .entry(editor.key.clone())
                .or_insert(Self::run_chain(chain));
        }

        if menu.offers_text_actions() {
            for (idx, act) in config.text_actions.iter().enumerate() {
                let action = WorkFlowAction::TextAction(TextAction::UserDefined(idx));
                items
                    .entry(act.key.clone())
                    .or_insert(Self::run_chain([action]));
            }
        }

        Self::add_workflows(&mut items, menu.unbound_workflows(config));

        let max_key_len = items.keys().map(|k| k.chars().count()).max().unwrap_or(0);
        (items, max_key_len)
    }

    /// Register `workflows` as menu entries: a row bound to a menu runs quietly,
    /// an unbound workflow is checked against the selection when its key is
    /// pressed and announces when it is done.
    fn add_workflows<'a>(
        items: &mut HashMap<String, AppSignal>,
        workflows: impl Iterator<Item = (usize, &'a WorkFlow)>,
    ) {
        for (idx, wf) in workflows {
            let signal = match wf.menu {
                Some(_) => Self::run_chain(wf.actions.as_slice()),
                None => AppSignal::RunWorkFlow(idx),
            };
            items.entry(wf.key.clone()).or_insert(signal);
        }
    }

    pub fn new(sender: Sender<AppSignal>, config: &GlyphlowConfig) -> KeyListener {
        let mut menu_actions = HashMap::new();
        let mut menu_action_max_key_len = HashMap::new();

        for menu in [
            MenuKind::Dashboard,
            MenuKind::TextAction,
            MenuKind::ImageAction,
            MenuKind::Scroll,
        ]
        .into_iter()
        {
            let (items, max_key_len) = Self::build_menu_actions(menu, config);
            menu_actions.insert(menu, items);
            menu_action_max_key_len.insert(menu, max_key_len);
        }

        KeyListener {
            menu_actions,
            menu_action_max_key_len,
            sender,
            global_key_binding: config.global_trigger_key.clone(),
        }
    }

    fn send(&self, signal: AppSignal) {
        if let Err(e) = self.sender.blocking_send(signal) {
            log::error!("Error sending signal: {}", e);
        }
    }

    fn handle_menu_key(
        &self,
        key: &Key,
        menu_type: MenuKind,
        mut state: MutexGuard<'_, Mode>,
        key_state: &mut KeyState,
    ) -> bool {
        let key_char = key.to_char();
        if *key == Key::Backspace || *key == Key::Delete {
            if *state == Mode::DictionaryScrolling && key_state.prefix.is_empty() {
                self.send(AppSignal::BackToTextActionMenu);
            } else {
                key_state.pop();
            }
        } else if key_state.prefix.chars().count() < self.menu_action_max_key_len[&menu_type] {
            key_state.push(key_char);
        }

        if let Some(signal) = self
            .menu_actions
            .get(&menu_type)
            .and_then(|m| m.get(&key_state.prefix))
        {
            self.send(signal.clone());
            key_state.clear_prefix();
        } else if key_char == ' ' {
            *state = Mode::Idle;
            self.send(AppSignal::DeActivate);
            key_state.clear_prefix();
        } else {
            if *state != Mode::DictionaryScrolling {
                let signal = match menu_type {
                    MenuKind::Dashboard => AppSignal::DashboardRefresh(key_state.prefix.clone()),
                    _ => AppSignal::MenuRefresh(key_state.prefix.clone()),
                };
                self.send(signal);
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
    pub fn key_down(&self, key: Key, state: &Mutex<Mode>, key_state: &mut KeyState) -> bool {
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
                    self.send(AppSignal::DashboardRefresh("".into()));
                    *state = Mode::DashBoard;
                    true
                } else {
                    false
                }
            }
            Mode::DashBoard => self.handle_menu_key(&key, MenuKind::Dashboard, state, key_state),
            Mode::WordPicking => self.handle_filter_key(&key, state, FilterMode::WordPicking),
            Mode::Filtering => self.handle_filter_key(&key, state, FilterMode::Generic),
            Mode::OCRResultFiltering => self.handle_filter_key(&key, state, FilterMode::OCR),
            Mode::TextActionMenu => {
                self.handle_menu_key(&key, MenuKind::TextAction, state, key_state)
            }
            Mode::ImageActionMenu => {
                self.handle_menu_key(&key, MenuKind::ImageAction, state, key_state)
            }
            Mode::Scrolling | Mode::DictionaryScrolling => {
                self.handle_menu_key(&key, MenuKind::Scroll, state, key_state)
            }
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
    pub prefix: String,
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

    pub fn clear_prefix(&mut self) {
        self.prefix.clear();
    }

    pub fn push(&mut self, key_char: char) {
        self.prefix.push(key_char);
    }

    pub fn pop(&mut self) {
        self.prefix.pop();
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
