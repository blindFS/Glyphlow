use super::AppEngine;
use crate::{
    AppSignal, Mode, ScrollAction, TextAction, ax_element::Target, config::RoleOfInterest,
};

/// Which menu is up, and therefore which rows a key is resolved against.
///
/// The scope is carried by [`Mode::Menu`] rather than derived from the selection
/// on every key: whoever draws a menu records what it drew, so the drawing and
/// the key resolution cannot disagree.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MenuScope {
    /// Nothing picked yet: targets, and workflows that apply anywhere.
    Dashboard,
    Text,
    Image,
    Scroll,
}

/// One keyed row of a menu: what the key is, what it shows and what it runs.
pub(super) struct MenuRow<'a> {
    pub(super) key: &'a str,
    pub(super) display: &'a str,
    pub(super) action: AppSignal,
}

impl MenuScope {
    /// The menu's title line.
    pub(super) fn header(self) -> &'static str {
        match self {
            Self::Dashboard => "Pick a Target:",
            Self::Text => "Pick an Action for Text:",
            Self::Image => "Pick an Action for Image:",
            Self::Scroll => "Pick a Scrolling Action:",
        }
    }

    /// Whether the scope offers the configured workflows on top of its own rows.
    fn offers_workflows(self) -> bool {
        self != Self::Scroll
    }

    fn editor_action(self) -> Option<AppSignal> {
        match self {
            Self::Dashboard => Some(AppSignal::Activate(Target::Edit)),
            Self::Text => Some(AppSignal::TextAction(TextAction::Editor)),
            Self::Image | Self::Scroll => None,
        }
    }

    fn builtin_rows(self) -> &'static [MenuRow<'static>] {
        match self {
            Self::Dashboard => DASHBOARD_ROWS,
            Self::Text => TEXT_ROWS,
            Self::Image => IMAGE_ROWS,
            Self::Scroll => SCROLL_ROWS,
        }
    }
}

const DASHBOARD_ROWS: &[MenuRow<'static>] = &[
    MenuRow {
        key: "T",
        display: "󰦨 Text",
        action: AppSignal::Activate(Target::Text),
    },
    MenuRow {
        key: "P",
        display: "󰳽 Press",
        action: AppSignal::Activate(Target::Clickable),
    },
    MenuRow {
        key: "S",
        display: "󱕒 ScrollBar",
        action: AppSignal::Activate(Target::Scrollable),
    },
    MenuRow {
        key: "I",
        display: "󰊄 Input",
        action: AppSignal::Activate(Target::Editable),
    },
    MenuRow {
        key: "M",
        display: "󰋩 Image",
        action: AppSignal::Activate(Target::Image),
    },
    MenuRow {
        key: "E",
        display: "󰙅 Element Explorer",
        action: AppSignal::Activate(Target::ChildElement),
    },
    MenuRow {
        key: "O",
        display: "󱄺 Image OCR",
        action: AppSignal::FrameOCR,
    },
];

const TEXT_ROWS: &[MenuRow<'static>] = &[
    MenuRow {
        key: "C",
        display: "⮺ Copy",
        action: AppSignal::TextAction(TextAction::Copy),
    },
    MenuRow {
        key: "D",
        display: "◫ Dictionary",
        action: AppSignal::TextAction(TextAction::Dictionary),
    },
    MenuRow {
        key: "S",
        display: "󰃻 Split",
        action: AppSignal::TextAction(TextAction::Split),
    },
];

const IMAGE_ROWS: &[MenuRow<'static>] = &[MenuRow {
    key: "O",
    display: "󱄺 Image OCR",
    action: AppSignal::FrameOCR,
}];

const SCROLL_ROWS: &[MenuRow<'static>] = &[
    MenuRow {
        key: "J",
        display: "> Down/Right",
        action: AppSignal::ScrollAction(ScrollAction::DownRight),
    },
    MenuRow {
        key: "K",
        display: "< Up/Left",
        action: AppSignal::ScrollAction(ScrollAction::UpLeft),
    },
    MenuRow {
        key: "I",
        display: "+ Distance Increase",
        action: AppSignal::ScrollAction(ScrollAction::IncreaseDistance),
    },
    MenuRow {
        key: "D",
        display: "- Distance Decrease",
        action: AppSignal::ScrollAction(ScrollAction::DecreaseDistance),
    },
    MenuRow {
        key: "GG",
        display: "󰢦 Top",
        action: AppSignal::ScrollAction(ScrollAction::Top),
    },
    MenuRow {
        key: "󰘶G",
        display: "󰢢 Bottom",
        action: AppSignal::ScrollAction(ScrollAction::Bottom),
    },
];

impl AppEngine {
    /// The menu a selection of `role` calls for.
    fn menu_scope_for(&self, role: RoleOfInterest) -> MenuScope {
        match role {
            RoleOfInterest::Image => MenuScope::Image,
            RoleOfInterest::ScrollBar => MenuScope::Scroll,
            RoleOfInterest::TextField | RoleOfInterest::StaticText | RoleOfInterest::PseudoText => {
                MenuScope::Text
            }
            _ if self.target == Target::Text => MenuScope::Text,
            _ if self.target == Target::Scrollable => MenuScope::Scroll,
            _ => MenuScope::Dashboard,
        }
    }

    /// Whether `scope` has a menu to show at all.
    pub(super) fn menu_visible(&self, scope: MenuScope) -> bool {
        scope != MenuScope::Scroll || !self.config.hide_scrolling_menu
    }

    /// The rows `scope` offers the current selection that `prefix` can still
    /// reach, in resolution order.
    pub(super) fn menu_rows<'a>(
        &'a self,
        scope: MenuScope,
        prefix: &'a str,
    ) -> impl Iterator<Item = MenuRow<'a>> + Clone + 'a {
        let offers_workflows = scope.offers_workflows();
        let workflows = self
            .config
            .workflows
            .iter()
            .enumerate()
            // Prefix before applicability: the prefix is the cheaper test, and a
            // row it cannot complete never needs the dearer one.
            .filter(move |(_, wf)| {
                offers_workflows && wf.key.starts_with(prefix) && self.is_workflow_valid(wf)
            })
            .map(|(idx, wf)| MenuRow {
                key: &wf.key,
                display: &wf.display,
                action: AppSignal::RunWorkFlow(idx),
            });

        let text_actions = (scope == MenuScope::Text)
            .then(|| {
                self.config
                    .text_actions
                    .iter()
                    .enumerate()
                    .map(|(idx, action)| MenuRow {
                        key: &action.key,
                        display: &action.display,
                        action: AppSignal::TextAction(TextAction::UserDefined(idx)),
                    })
            })
            .into_iter()
            .flatten();

        let editor = self
            .config
            .editor
            .as_ref()
            .zip(scope.editor_action())
            .into_iter()
            .map(|(editor, action)| MenuRow {
                key: &editor.key,
                display: &editor.display,
                action,
            });

        let builtins = scope.builtin_rows().iter().map(|row| MenuRow {
            key: row.key,
            display: row.display,
            action: row.action.clone(),
        });

        builtins
            .chain(text_actions)
            .chain(editor)
            .chain(workflows)
            .filter(move |row| row.key.starts_with(prefix))
    }

    /// A character typed while a menu is up.
    pub(super) fn menu_key(&mut self, key_char: char) {
        let (scope, drawn) = match self.mode() {
            Mode::Menu(scope) => (scope, true),
            Mode::DictionaryScrolling => (MenuScope::Scroll, false),
            _ => return,
        };

        self.menu_prefix.push(key_char);

        // One filtered row list serves both the decision and the drawing, and it
        // is dropped before the prefix is touched: it borrows `self`.
        let (action, reachable) = {
            let mut rows = self.menu_rows(scope, &self.menu_prefix);

            // Exact before prefix: a row whose key is the prefix itself owns it,
            // even when a longer row starts with the same keys. The cost is that
            // a key which is a proper prefix of another's cannot be reached; no
            // menu ships such a pair.
            match rows.clone().find(|row| row.key == self.menu_prefix) {
                Some(row) => (Some(row.action), true),
                None if drawn => (None, self.show_menu_rows(scope, &self.menu_prefix, rows)),
                None => (None, rows.next().is_some()),
            }
        };

        match action {
            Some(action) => {
                self.menu_prefix.clear();
                self.run_menu_action(action);
            }
            // A key that reaches no row is not kept: the prefix is left alone.
            None if !reachable => {
                self.menu_prefix.pop();
            }
            None => (),
        }
    }

    /// Backspace while a menu is up: undo one key, or go back a menu level.
    pub(super) fn menu_backspace(&mut self) {
        self.menu_prefix.pop();
        match self.mode() {
            Mode::Menu(scope) => self.show_menu(scope, &self.menu_prefix),
            Mode::DictionaryScrolling => self.show_menu(MenuScope::Text, ""),
            _ => (),
        }
    }

    fn run_menu_action(&self, action: AppSignal) {
        if let Err(e) = self.signal_sender.try_send(action) {
            log::error!("Failed to queue menu action: {e}");
        }
    }

    pub(super) fn show_element_menu(&self, role: RoleOfInterest) {
        self.show_menu(self.menu_scope_for(role), "");
    }
}
