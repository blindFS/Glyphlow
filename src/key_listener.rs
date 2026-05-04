use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    path::PathBuf,
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
    // State signals
    DashBoard,
    Activate(Target),
    DeActivate,
    Filter(char, FilterMode),
    // Sub state signals
    FileUpdate(PathBuf),
    ClearNotification,
    ToggleMultiSelection,
    // Menu specific
    TextAction(TextAction),
    ScrollAction(ScrollAction),
    // Generic Actions
    RunWorkFlow(usize),
    ReadClipboard,
    ScreenShot,
    FrameOCR,
    Press,
    ShowMenu,
}

#[derive(Debug, PartialEq)]
pub struct MenuItem {
    pub description: &'static str,
    pub key: char,
    pub action: AppSignal,
}

impl MenuItem {
    pub const fn new(description: &'static str, key: char, action: AppSignal) -> MenuItem {
        MenuItem {
            description,
            key,
            action,
        }
    }
}

impl Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}) {}", self.key, self.description)
    }
}

// TODO: Config sub-menu to
// 1. Reload config
// 2. Toggle aggressive visibility check
pub const DASH_BOARD_MENU_ITEMS: [MenuItem; 9] = [
    MenuItem::new("󰦨 Text", 'T', AppSignal::Activate(Target::Text)),
    MenuItem::new("󰳽 Press", 'P', AppSignal::Activate(Target::Clickable)),
    MenuItem::new("󱕒 ScrollBar", 'S', AppSignal::Activate(Target::Scrollable)),
    MenuItem::new("󰊄 Input", 'I', AppSignal::Activate(Target::Editable)),
    MenuItem::new(" Image", 'M', AppSignal::Activate(Target::Image)),
    MenuItem::new("󰙅 Element", 'E', AppSignal::Activate(Target::ChildElement)),
    MenuItem::new("󰆟 ScreenShot", 'R', AppSignal::ScreenShot),
    MenuItem::new("󱄺 Image OCR", 'O', AppSignal::FrameOCR),
    MenuItem::new(" Read Clipboard", 'C', AppSignal::ReadClipboard),
];

pub const SCROLLBAR_MENU_ITEMS: [MenuItem; 4] = [
    MenuItem::new(
        "> Down/Right",
        'J',
        AppSignal::ScrollAction(ScrollAction::DownRight),
    ),
    MenuItem::new(
        "< Up/Left",
        'K',
        AppSignal::ScrollAction(ScrollAction::UpLeft),
    ),
    MenuItem::new(
        "+ Distance Increase",
        'I',
        AppSignal::ScrollAction(ScrollAction::IncreaseDistance),
    ),
    MenuItem::new(
        "- Distance Decrease",
        'D',
        AppSignal::ScrollAction(ScrollAction::DecreaseDistance),
    ),
];

pub const TEXT_ACTION_MENU_ITEMS: [MenuItem; 5] = [
    MenuItem::new("⮺ Copy", 'C', AppSignal::TextAction(TextAction::Copy)),
    MenuItem::new(
        "◫ Dictionary",
        'D',
        AppSignal::TextAction(TextAction::Dictionary),
    ),
    MenuItem::new("󰃻 Split", 'S', AppSignal::TextAction(TextAction::Split)),
    MenuItem::new("󰳽 Press [Left Click]", 'P', AppSignal::Press),
    MenuItem::new(" Menu [Right Click]", 'M', AppSignal::ShowMenu),
];

pub const IMAGE_ACTION_MENU_ITEMS: [MenuItem; 3] = [
    MenuItem::new("󱄺 Image OCR", 'O', AppSignal::FrameOCR),
    MenuItem::new("󰳽 Press [Left Click]", 'P', AppSignal::Press),
    MenuItem::new(" Menu [Right Click]", 'M', AppSignal::ShowMenu),
];

#[derive(Debug, PartialEq)]
pub enum Mode {
    DashBoard,
    Filtering,
    Idle,
    Scrolling,
    TextActionMenu,
    ImageActionMenu,
    Editing,
    WordPicking,
    OCRResultFiltering,
    WaitAndDeactivate,
}

#[derive(Debug)]
pub struct KeyListener {
    pub text_actions: HashMap<char, AppSignal>,
    pub image_actions: HashMap<char, AppSignal>,
    pub dashboard_actions: HashMap<char, AppSignal>,
    pub scroll_actions: HashMap<char, AppSignal>,
    sender: Sender<AppSignal>,
    global_key_binding: KeyBinding,
}

impl KeyListener {
    fn iter_from<const N: usize>(items: [MenuItem; N]) -> impl Iterator<Item = (char, AppSignal)> {
        items.into_iter().map(|it| (it.key, it.action))
    }

    pub fn new(sender: Sender<AppSignal>, config: &GlyphlowConfig) -> KeyListener {
        let mut text_actions =
            // Order matters!
            config
                .workflows
                .iter()
                .enumerate()
                .map(|(idx, wf)| (wf.key, AppSignal::RunWorkFlow(idx)))
                .chain(
                    config.text_actions.iter().enumerate().map(|(idx, ca)| {
                        (ca.key, AppSignal::TextAction(TextAction::UserDefined(idx)))
                    }),
                )
                .chain(Self::iter_from(TEXT_ACTION_MENU_ITEMS))
                .collect::<HashMap<_, _>>();

        let mut dashboard_actions = config
            .workflows
            .iter()
            .enumerate()
            .map(|(idx, wf)| (wf.key, AppSignal::RunWorkFlow(idx)))
            .chain(Self::iter_from(DASH_BOARD_MENU_ITEMS))
            .collect::<HashMap<_, _>>();

        let image_actions = config
            .workflows
            .iter()
            .enumerate()
            .map(|(idx, wf)| (wf.key, AppSignal::RunWorkFlow(idx)))
            .chain(Self::iter_from(IMAGE_ACTION_MENU_ITEMS))
            .collect::<HashMap<_, _>>();

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
            image_actions,
            dashboard_actions,
            scroll_actions,
            sender,
            global_key_binding: config.global_trigger_key.clone(),
        }
    }

    fn send(&self, signal: AppSignal) {
        if let Err(e) = self.sender.blocking_send(signal) {
            log::error!("Error sending signal: {}", e);
        }
    }

    fn helper(
        &self,
        key: &Key,
        key_signals: &HashMap<char, AppSignal>,
        mut state: MutexGuard<'_, Mode>,
    ) -> bool {
        let key_char = key.to_char();
        if let Some(signal) = key_signals.get(&key_char) {
            self.send(signal.clone());
        } else if key_char == ' ' {
            self.send(AppSignal::DeActivate);
            *state = Mode::Idle;
        }
        true
    }

    fn filter_helper(&self, key: &Key, mut state: MutexGuard<'_, Mode>, mode: FilterMode) -> bool {
        let key_char = key.to_char();
        if key_char == ' ' {
            self.send(AppSignal::DeActivate);
            *state = Mode::Idle;
        } else {
            self.send(AppSignal::Filter(key_char, mode));
        }
        true
    }

    /// Returns true if key is effective, and should be swallowed by this app
    pub fn key_down(&self, key: Key, state: &Mutex<Mode>, pressed_keys: &HashSet<Key>) -> bool {
        let Ok(mut state) = state.try_lock() else {
            return false;
        };

        match *state {
            Mode::Editing | Mode::Idle => {
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
            Mode::DashBoard => self.helper(&key, &self.dashboard_actions, state),
            // To act on selected parent node
            Mode::Filtering if key == Key::Return => {
                self.send(AppSignal::DashBoard);
                *state = Mode::DashBoard;
                true
            }
            Mode::WordPicking | Mode::Filtering | Mode::OCRResultFiltering
                if key == Key::ShiftLeft || key == Key::ShiftRight =>
            {
                self.send(AppSignal::ToggleMultiSelection);
                true
            }
            Mode::WordPicking => self.filter_helper(&key, state, FilterMode::WordPicking),
            Mode::Filtering => self.filter_helper(&key, state, FilterMode::Generic),
            Mode::OCRResultFiltering => self.filter_helper(&key, state, FilterMode::OCR),
            Mode::TextActionMenu => self.helper(&key, &self.text_actions, state),
            Mode::ImageActionMenu => self.helper(&key, &self.image_actions, state),
            Mode::Scrolling => self.helper(&key, &self.scroll_actions, state),
            Mode::WaitAndDeactivate => {
                self.send(AppSignal::DeActivate);
                *state = Mode::Idle;
                true
            }
        }
    }
}
