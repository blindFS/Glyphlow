use super::AppEngine;
use crate::{
    AppSignal, Mode, ScrollAction, TextAction, ax_element::Target, config::RoleOfInterest,
};
use std::collections::HashSet;

/// Which menu is up, and therefore which rows a key is resolved against.
///
/// The scope is carried by [`Mode::Menu`] rather than derived from the selection
/// on every key: whoever draws a menu records what it drew, so the drawing and
/// the key resolution cannot disagree.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MenuScope {
    /// Nothing picked yet: targets, and workflows that apply anywhere.
    Dashboard,
    /// A text selection: text actions, the editor and text workflows.
    Text,
    /// A picked image.
    Image,
    /// A scroll bar, or a scrollable element scrolled by wheel.
    Scroll,
}

/// One keyed row of a menu: what the key is, what it shows and what it runs.
pub(super) struct MenuRow<'a> {
    pub(super) key: &'a str,
    pub(super) display: &'a str,
    pub(super) action: AppSignal,
}

/// What a key sequence resolves to.
enum Resolution {
    /// A row owns this prefix: run it.
    Exact(AppSignal),
    /// Some row still starts with this prefix: wait for more keys.
    Prefix,
    /// No row can complete this prefix.
    Dead,
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

    /// What the editor row runs in this menu, if it has one.
    ///
    /// The same key means two things: the dashboard opens the editor on a text
    /// element it still has to find, the text menu edits the selection at hand.
    fn editor_action(self) -> Option<AppSignal> {
        match self {
            Self::Dashboard => Some(AppSignal::Activate(Target::Edit)),
            Self::Text => Some(AppSignal::TextAction(TextAction::Editor)),
            Self::Image | Self::Scroll => None,
        }
    }

    /// The rows this menu offers whatever the selection is.
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

/// Keep the first row to claim a key, so a menu shows one row per key.
///
/// Dropping the later rows is what keeps the drawing and the resolution in step:
/// a key the menu does not show cannot be reached, and a key it does show always
/// reaches the row printed next to it.
fn first_claim_wins(rows: Vec<MenuRow<'_>>) -> Vec<MenuRow<'_>> {
    let mut claimed = HashSet::new();
    rows.into_iter()
        .filter(|row| {
            let first = claimed.insert(row.key);
            if !first {
                log::debug!("Menu key `{}` of `{}` is shadowed", row.key, row.display);
            }
            first
        })
        .collect()
}

impl AppEngine {
    /// The menu a selection of `role` calls for.
    ///
    /// A text or scroll activation falls back to its own menu even when the
    /// picked element's role would have offered the dashboard.
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

    /// The rows `scope` offers the current selection, in resolution order.
    ///
    /// Applicability is applied here, once: a row that cannot run is not in the
    /// list, so neither the drawing nor a key press can reach it.
    pub(super) fn menu_rows(&self, scope: MenuScope) -> Vec<MenuRow<'_>> {
        let mut rows = Vec::new();

        if scope.offers_workflows() {
            for (idx, wf) in self.config.workflows.iter().enumerate() {
                if self.is_workflow_valid(wf) {
                    rows.push(MenuRow {
                        key: &wf.key,
                        display: &wf.display,
                        action: AppSignal::RunWorkFlow(idx),
                    });
                }
            }
        }

        if scope == MenuScope::Text {
            for (idx, action) in self.config.text_actions.iter().enumerate() {
                rows.push(MenuRow {
                    key: &action.key,
                    display: &action.display,
                    action: AppSignal::TextAction(TextAction::UserDefined(idx)),
                });
            }
        }

        if let Some(editor) = self.config.editor.as_ref()
            && let Some(action) = scope.editor_action()
        {
            rows.push(MenuRow {
                key: &editor.key,
                display: &editor.display,
                action,
            });
        }

        for row in scope.builtin_rows() {
            rows.push(MenuRow {
                key: row.key,
                display: row.display,
                action: row.action.clone(),
            });
        }

        first_claim_wins(rows)
    }

    /// What `prefix` means in `scope`'s menu.
    ///
    /// An exact match wins over a longer row that merely starts with the same
    /// keys, which is what lets a menu mix single and multi key rows. The cost
    /// is that a row whose key is a proper prefix of another's can never be
    /// reached; no menu ships such a pair.
    fn resolve(&self, scope: MenuScope, prefix: &str) -> Resolution {
        let rows = self.menu_rows(scope);
        if let Some(row) = rows.iter().find(|row| row.key == prefix) {
            return Resolution::Exact(row.action.clone());
        }
        if rows.iter().any(|row| row.key.starts_with(prefix)) {
            Resolution::Prefix
        } else {
            Resolution::Dead
        }
    }

    /// A character typed while a menu is up.
    ///
    /// A dictionary answers to the same keys without being a menu: it resolves
    /// them, but has nothing to redraw around a prefix.
    pub(super) fn menu_key(&mut self, key_char: char) {
        let (scope, drawn) = match self.mode() {
            Mode::Menu(scope) => (scope, true),
            Mode::DictionaryScrolling => (MenuScope::Scroll, false),
            _ => return,
        };

        // Typed and untyped in place: a key that reaches no row leaves the
        // prefix exactly as it found it, so another key can still complete it
        // and backspace can still undo it.
        self.menu_prefix.push(key_char);
        match self.resolve(scope, &self.menu_prefix) {
            Resolution::Exact(action) => {
                self.menu_prefix.clear();
                self.run_menu_action(action);
            }
            Resolution::Prefix if drawn => self.show_menu(scope, &self.menu_prefix),
            Resolution::Prefix => (),
            Resolution::Dead => {
                self.menu_prefix.pop();
                if drawn {
                    self.show_wrong_key(scope);
                }
            }
        }
    }

    /// Backspace while a menu is up: undo one key, or go back a menu level.
    pub(super) fn menu_backspace(&mut self) {
        match self.mode() {
            Mode::Menu(scope) => {
                self.menu_prefix.pop();
                self.show_menu(scope, &self.menu_prefix);
            }
            Mode::DictionaryScrolling => self.leave_dictionary(),
            _ => (),
        }
    }

    /// Backspace on a dictionary: drop a key if one was typed, otherwise give
    /// way to the text actions it was opened from.
    fn leave_dictionary(&mut self) {
        if self.menu_prefix.pop().is_none() {
            self.show_menu(MenuScope::Text, "");
        }
    }

    /// Queue the action a row resolved to.
    ///
    /// Going through the queue keeps one dispatch path for menu keys and for
    /// every other signal; the engine cannot await its own handler.
    fn run_menu_action(&self, action: AppSignal) {
        if let Err(e) = self.signal_sender.try_send(action) {
            log::error!("Failed to queue menu action: {e}");
        }
    }

    /// Draw the menu `role` calls for.
    pub(super) fn show_element_menu(&self, role: RoleOfInterest) {
        self.show_menu(self.menu_scope_for(role), "");
    }
}
