use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    sync::{Mutex, MutexGuard},
};

use rdev::Key;
use tokio::sync::mpsc::Sender;

use crate::{
    ax_element::Target,
    config::{AlphabeticKey, GlyphlowConfig, KeyBinding},
};

#[derive(Debug, PartialEq, Clone)]
pub enum TextAction {
    Copy,
    Dictionary,
    Split,
    Editor,
    /// index of the action in the config
    UserDefined(usize),
}

#[derive(Debug, PartialEq, Clone)]
pub enum ScrollAction {
    UpLeft,
    DownRight,
    IncreaseDistance,
    DecreaseDistance,
}

#[derive(Debug, PartialEq, Clone)]
pub enum FilterMode {
    WordPicking,
    Generic,
    OCR,
}

#[derive(Debug, PartialEq, Clone)]
pub enum AppSignal {
    DashBoard,
    Activate(Target),
    DeActivate,
    Filter(char, FilterMode),
    TextAction(TextAction),
    ScrollAction(ScrollAction),
    ReadClipboard,
    ScreenShot,
    FileUpdate,
    ClearNotification,
}

#[derive(Debug, PartialEq)]
pub struct StaticMenuItem {
    pub description: &'static str,
    pub key: char,
    pub action: AppSignal,
}

impl StaticMenuItem {
    pub const fn new(description: &'static str, key: char, action: AppSignal) -> StaticMenuItem {
        StaticMenuItem {
            description,
            key,
            action,
        }
    }
}

impl Display for StaticMenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.description, self.key)
    }
}

// TODO: Config sub-menu to
// 1. Reload config
// 2. Toggle aggressive visibility check
pub const DASH_BOARD_MENU_ITEMS: [StaticMenuItem; 9] = [
    StaticMenuItem::new("󰦨 Text", 'T', AppSignal::Activate(Target::Text)),
    StaticMenuItem::new("󰳽 Press", 'P', AppSignal::Activate(Target::Clickable)),
    StaticMenuItem::new("󱕒 ScrollBar", 'S', AppSignal::Activate(Target::ScrollBar)),
    StaticMenuItem::new("󰊄 Input", 'I', AppSignal::Activate(Target::Editable)),
    StaticMenuItem::new(" Image", 'M', AppSignal::Activate(Target::Image)),
    StaticMenuItem::new("󰙅 Element", 'E', AppSignal::Activate(Target::ChildElement)),
    StaticMenuItem::new("󰆟 ScreenShot", 'R', AppSignal::ScreenShot),
    StaticMenuItem::new("󱄺 Image OCR", 'O', AppSignal::Activate(Target::ImageOCR)),
    StaticMenuItem::new(" Read Clipboard", 'C', AppSignal::ReadClipboard),
];

pub const SCROLLBAR_MENU_ITEMS: [StaticMenuItem; 4] = [
    StaticMenuItem::new(
        "> Down/Right",
        'J',
        AppSignal::ScrollAction(ScrollAction::DownRight),
    ),
    StaticMenuItem::new(
        "< Up/Left",
        'K',
        AppSignal::ScrollAction(ScrollAction::UpLeft),
    ),
    StaticMenuItem::new(
        "+ Distance Increase",
        'I',
        AppSignal::ScrollAction(ScrollAction::IncreaseDistance),
    ),
    StaticMenuItem::new(
        "- Distance Decrease",
        'D',
        AppSignal::ScrollAction(ScrollAction::DecreaseDistance),
    ),
];

pub const TEXT_ACTION_MENU_ITEMS: [StaticMenuItem; 3] = [
    StaticMenuItem::new("⮺ Copy", 'C', AppSignal::TextAction(TextAction::Copy)),
    StaticMenuItem::new(
        "◫ Dictionary",
        'D',
        AppSignal::TextAction(TextAction::Dictionary),
    ),
    StaticMenuItem::new("󰃻 Split", 'S', AppSignal::TextAction(TextAction::Split)),
];

#[derive(Debug, PartialEq)]
pub enum Mode {
    DashBoard,
    Filtering,
    Idle,
    Scrolling,
    TextActionMenu,
    Transparent,
    WordPicking,
    OCRResultFiltering,
    Notification,
}

#[derive(Debug)]
pub struct KeyListener {
    pub text_actions: HashMap<char, AppSignal>,
    pub dashboard_actions: HashMap<char, AppSignal>,
    pub scroll_actions: HashMap<char, AppSignal>,
    sender: Sender<AppSignal>,
    global_key_binding: KeyBinding,
}

impl KeyListener {
    fn iter_from<const N: usize>(
        items: [StaticMenuItem; N],
    ) -> impl Iterator<Item = (char, AppSignal)> {
        items.into_iter().map(|it| (it.key, it.action))
    }

    pub fn new(sender: Sender<AppSignal>, config: &GlyphlowConfig) -> KeyListener {
        let mut text_actions =
            Self::iter_from(TEXT_ACTION_MENU_ITEMS)
                .chain(
                    config.text_actions.iter().enumerate().map(|(idx, ca)| {
                        (ca.key, AppSignal::TextAction(TextAction::UserDefined(idx)))
                    }),
                )
                .collect::<HashMap<_, _>>();
        let mut dashboard_actions =
            Self::iter_from(DASH_BOARD_MENU_ITEMS).collect::<HashMap<_, _>>();
        let scroll_actions = Self::iter_from(SCROLLBAR_MENU_ITEMS).collect::<HashMap<_, _>>();

        if let Some(editor_command) = config.editor.as_ref() {
            text_actions.insert(
                editor_command.key,
                AppSignal::TextAction(TextAction::Editor),
            );
            dashboard_actions.insert(editor_command.key, AppSignal::Activate(Target::Edit));
        }

        KeyListener {
            text_actions,
            dashboard_actions,
            scroll_actions,
            sender,
            global_key_binding: config.global_trigger_key.clone(),
        }
    }

    pub fn is_active(&self, state: &Mutex<Mode>) -> bool {
        if let Ok(state) = state.try_lock() {
            *state != Mode::Idle && *state != Mode::Transparent
        } else {
            false
        }
    }

    fn send(&self, signal: AppSignal) {
        if let Err(e) = self.sender.blocking_send(signal) {
            eprintln!("Error sending signal: {}", e);
        }
    }

    /// Returns true if key is effective, and should be swallowed by this app
    pub fn key_down(&self, key: Key, state: &Mutex<Mode>, pressed_keys: &HashSet<Key>) -> bool {
        let Ok(mut state) = state.try_lock() else {
            return false;
        };
        let key_char = key.to_char();

        let helper =
            |key_signals: &HashMap<char, AppSignal>, mut state: MutexGuard<'_, Mode>| -> bool {
                if let Some(signal) = key_signals.get(&key_char) {
                    self.send(signal.clone());
                } else {
                    self.send(AppSignal::DeActivate);
                    *state = Mode::Idle;
                }
                true
            };

        let filter_helper =
            |key_char: char, mut state: MutexGuard<'_, Mode>, mode: FilterMode| -> bool {
                if key_char == ' ' {
                    self.send(AppSignal::DeActivate);
                    *state = Mode::Idle;
                } else {
                    self.send(AppSignal::Filter(key_char, mode));
                }
                true
            };

        match *state {
            Mode::Idle => {
                if self.global_key_binding.keys.iter().all(|k| {
                    k == &key
                        || pressed_keys.contains(k)
                        || k.right_alternative()
                            .is_some_and(|r| *k == r || pressed_keys.contains(&r))
                }) {
                    self.send(AppSignal::DashBoard);
                    *state = Mode::DashBoard;
                    true
                } else {
                    false
                }
            }
            Mode::DashBoard => helper(&self.dashboard_actions, state),
            // To act on selected parent node
            Mode::Filtering if key == Key::Return => {
                self.send(AppSignal::DashBoard);
                *state = Mode::DashBoard;
                true
            }
            Mode::WordPicking => filter_helper(key_char, state, FilterMode::WordPicking),
            Mode::Filtering => filter_helper(key_char, state, FilterMode::Generic),
            Mode::OCRResultFiltering => filter_helper(key_char, state, FilterMode::OCR),
            Mode::TextActionMenu => helper(&self.text_actions, state),
            Mode::Scrolling => helper(&self.scroll_actions, state),
            Mode::Notification => {
                self.send(AppSignal::DeActivate);
                *state = Mode::Idle;
                true
            }
            // Transparent mode
            _ => false,
        }
    }
}
