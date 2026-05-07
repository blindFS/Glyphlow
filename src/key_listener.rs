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
    Top,
    Bottom,
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
    Activate(Target),
    DeActivate,
    Filter(char, FilterMode),
    MenuRefresh(String),
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
}

#[derive(Debug, PartialEq)]
pub struct MenuItem {
    pub description: &'static str,
    pub key: &'static str,
    pub action: AppSignal,
}

impl MenuItem {
    pub const fn new(description: &'static str, key: &'static str, action: AppSignal) -> MenuItem {
        MenuItem {
            description,
            key,
            action,
        }
    }
}

impl MenuItem {
    pub fn pretty_print(&self, prefix_len: usize) -> String {
        let prefix = "_".repeat(prefix_len);
        format!(
            "({prefix}{}) {}",
            self.key.chars().skip(prefix_len).collect::<String>(),
            self.description
        )
    }
}

impl Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}) {}", self.key, self.description)
    }
}

pub const DASH_BOARD_MENU_ITEMS: [MenuItem; 9] = [
    MenuItem::new("󰦨 Text", "T", AppSignal::Activate(Target::Text)),
    MenuItem::new("󰳽 Press", "P", AppSignal::Activate(Target::Clickable)),
    MenuItem::new("󱕒 ScrollBar", "S", AppSignal::Activate(Target::Scrollable)),
    MenuItem::new("󰊄 Input", "I", AppSignal::Activate(Target::Editable)),
    MenuItem::new(" Image", "M", AppSignal::Activate(Target::Image)),
    MenuItem::new(
        "󰙅 Element Explorer",
        "E",
        AppSignal::Activate(Target::ChildElement),
    ),
    MenuItem::new("󰆟 ScreenShot", "R", AppSignal::ScreenShot),
    MenuItem::new("󱄺 Image OCR", "O", AppSignal::FrameOCR),
    MenuItem::new(" Read Clipboard", "C", AppSignal::ReadClipboard),
];

pub const SCROLLBAR_MENU_ITEMS: [MenuItem; 6] = [
    MenuItem::new(
        "> Down/Right",
        "J",
        AppSignal::ScrollAction(ScrollAction::DownRight),
    ),
    MenuItem::new(
        "< Up/Left",
        "K",
        AppSignal::ScrollAction(ScrollAction::UpLeft),
    ),
    MenuItem::new(
        "+ Distance Increase",
        "I",
        AppSignal::ScrollAction(ScrollAction::IncreaseDistance),
    ),
    MenuItem::new(
        "- Distance Decrease",
        "D",
        AppSignal::ScrollAction(ScrollAction::DecreaseDistance),
    ),
    MenuItem::new("󰢦 Top", "GG", AppSignal::ScrollAction(ScrollAction::Top)),
    MenuItem::new(
        "󰢢 Bottom",
        "󰘶G",
        AppSignal::ScrollAction(ScrollAction::Bottom),
    ),
];

pub const TEXT_ACTION_MENU_ITEMS: [MenuItem; 3] = [
    MenuItem::new("⮺ Copy", "C", AppSignal::TextAction(TextAction::Copy)),
    MenuItem::new(
        "◫ Dictionary",
        "D",
        AppSignal::TextAction(TextAction::Dictionary),
    ),
    MenuItem::new("󰃻 Split", "S", AppSignal::TextAction(TextAction::Split)),
];

pub const IMAGE_ACTION_MENU_ITEMS: [MenuItem; 1] =
    [MenuItem::new("󱄺 Image OCR", "O", AppSignal::FrameOCR)];

#[derive(Debug, PartialEq, Clone)]
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

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum MenuType {
    Dashboard,
    TextAction,
    ImageAction,
    Scroll,
}

#[derive(Debug)]
pub struct KeyListener {
    menu_actions: HashMap<MenuType, HashMap<String, AppSignal>>,
    menu_action_max_key_len: HashMap<MenuType, usize>,
    sender: Sender<AppSignal>,
    global_key_binding: KeyBinding,
}

impl KeyListener {
    fn iter_from<const N: usize>(
        items: [MenuItem; N],
    ) -> impl Iterator<Item = (String, AppSignal)> {
        items.into_iter().map(|it| (it.key.to_string(), it.action))
    }

    fn menu_action_helper(
        menu_type: MenuType,
        config: &GlyphlowConfig,
    ) -> (HashMap<String, AppSignal>, usize) {
        let (base_items, need_workflow, need_text_action, editor_signal) = match menu_type {
            MenuType::Dashboard => (
                Self::iter_from(DASH_BOARD_MENU_ITEMS).collect::<HashMap<_, _>>(),
                true,
                false,
                Some(AppSignal::Activate(Target::Edit)),
            ),
            MenuType::TextAction => (
                Self::iter_from(TEXT_ACTION_MENU_ITEMS).collect::<HashMap<_, _>>(),
                true,
                true,
                Some(AppSignal::TextAction(TextAction::Editor)),
            ),
            MenuType::ImageAction => (
                Self::iter_from(IMAGE_ACTION_MENU_ITEMS).collect::<HashMap<_, _>>(),
                true,
                false,
                None,
            ),
            MenuType::Scroll => (
                Self::iter_from(SCROLLBAR_MENU_ITEMS).collect::<HashMap<_, _>>(),
                false,
                false,
                None,
            ),
        };

        let mut items = HashMap::new();
        // Order matters!
        if need_workflow {
            for (idx, wf) in config.workflows.iter().enumerate() {
                items.insert(wf.key.clone(), AppSignal::RunWorkFlow(idx));
            }
        }

        if need_text_action {
            for (idx, act) in config.text_actions.iter().enumerate() {
                items.insert(
                    act.key.clone(),
                    AppSignal::TextAction(TextAction::UserDefined(idx)),
                );
            }
        }

        if let Some(sig) = editor_signal
            && let Some(editor) = config.editor.as_ref()
        {
            items.insert(editor.key.clone(), sig);
        }

        for (key, sig) in base_items {
            items.insert(key, sig);
        }

        let max_key_len = items.keys().map(|k| k.chars().count()).max().unwrap_or(0);
        (items, max_key_len)
    }

    pub fn new(sender: Sender<AppSignal>, config: &GlyphlowConfig) -> KeyListener {
        let mut menu_actions = HashMap::new();
        let mut menu_action_max_key_len = HashMap::new();

        for menu_type in [
            MenuType::Dashboard,
            MenuType::TextAction,
            MenuType::ImageAction,
            MenuType::Scroll,
        ]
        .into_iter()
        {
            let (items, max_key_len) = Self::menu_action_helper(menu_type, config);
            menu_actions.insert(menu_type, items);
            menu_action_max_key_len.insert(menu_type, max_key_len);
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

    fn menu_helper(
        &self,
        key: &Key,
        menu_type: MenuType,
        mut state: MutexGuard<'_, Mode>,
        key_state: &mut KeyState,
    ) -> bool {
        let key_char = key.to_char();
        if key_char == '-' {
            key_state.pop();
        } else if key_state.prefix.chars().count() < self.menu_action_max_key_len[&menu_type] {
            key_state.push(key_char);
        }

        if let Some(signal) = self
            .menu_actions
            .get(&menu_type)
            .and_then(|m| m.get(&key_state.prefix))
        {
            if let AppSignal::Activate(_) = signal {
                *state = Mode::Filtering;
            }
            self.send(signal.clone());
            key_state.clear_prefix();
        } else if key_char == ' ' {
            *state = Mode::Idle;
            self.send(AppSignal::DeActivate);
            key_state.clear_prefix();
        } else {
            self.send(AppSignal::MenuRefresh(key_state.prefix.clone()));
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
    pub fn key_down(&self, key: Key, state: &Mutex<Mode>, key_state: &mut KeyState) -> bool {
        let Ok(mut state) = state.try_lock() else {
            return false;
        };

        match *state {
            Mode::Editing | Mode::Idle => {
                if self.global_key_binding.keys.iter().all(|k| {
                    k == &key
                        || key_state.pressed_keys.contains(k)
                        || k.right_alternative()
                            .is_some_and(|r| *k == r || key_state.pressed_keys.contains(&r))
                }) {
                    self.send(AppSignal::MenuRefresh("".into()));
                    *state = Mode::DashBoard;
                    true
                } else {
                    false
                }
            }
            Mode::DashBoard => self.menu_helper(&key, MenuType::Dashboard, state, key_state),
            // To act on selected parent node
            Mode::Filtering if key == Key::Return => {
                self.send(AppSignal::MenuRefresh("".into()));
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
            Mode::TextActionMenu => self.menu_helper(&key, MenuType::TextAction, state, key_state),
            Mode::ImageActionMenu => {
                self.menu_helper(&key, MenuType::ImageAction, state, key_state)
            }
            Mode::Scrolling => self.menu_helper(&key, MenuType::Scroll, state, key_state),
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
