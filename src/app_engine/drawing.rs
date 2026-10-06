use super::AppEngine;
use crate::{
    DASH_BOARD_MENU_ITEMS, IMAGE_ACTION_MENU_ITEMS, MenuItem, Mode, SCROLLBAR_MENU_ITEMS,
    TEXT_ACTION_MENU_ITEMS,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    user_interface::{MenuString, MenuStyle},
    util::search_regex,
};
use objc2::rc::autoreleasepool;

const MAX_TEXT_DISPLAY_LEN: usize = 30;

/// The selection as one line of at most [`MAX_TEXT_DISPLAY_LEN`] characters plus an
/// ellipsis. Whitespace runs collapse, so a selection cannot add lines to the menu.
fn preview_line(text: &str) -> String {
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        if line.chars().count() > MAX_TEXT_DISPLAY_LEN {
            break;
        }
    }

    match line.char_indices().nth(MAX_TEXT_DISPLAY_LEN) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

const BACK_KEY: &str = "󰁮";

fn back_hint(prefix: &str) -> MenuString {
    let mut menu = MenuString::from(prefix);
    menu.push_styled(BACK_KEY, MenuStyle::Key)
        .push(" to go back");
    menu
}

pub(super) fn wrong_key_sequence() -> MenuString {
    let mut menu = back_hint("Wrong key sequence\nPress ");
    menu.style_head(MenuStyle::Error);
    menu
}

impl AppEngine {
    pub(super) fn clear_drawing(&mut self) {
        self.clear_grid();
        self.drawer.clear();
    }

    /// Remove the hint layers. Only safe on deactivation or when a single match
    /// remains — both end hint filtering.
    pub(super) fn clear_hints(&self) {
        for hb in self.hint_boxes.iter() {
            hb.free();
        }
    }

    /// Set the selected element and draw its frame box.
    pub(super) fn select(&mut self, eoi: ElementOfInterest) {
        self.drawer.draw_frame(&eoi.frame);
        self.selected = Some(eoi);
    }

    pub(super) fn draw_hints(&mut self) {
        autoreleasepool(|_| {
            for hb in self.hint_boxes.iter_mut() {
                hb.draw(
                    &self.drawer.root,
                    &self.config.theme,
                    self.hint_prefix.len(),
                    &self.overlay_frame,
                );
            }
        })
    }

    /// Settle every label once the final hint count is known.
    ///
    /// The digit width grows mid-traversal (the 27th hint needs two characters).
    /// This runs *after* collision resolution on purpose:
    /// [`HintBox::set_label`](crate::user_interface::HintBox::set_label) re-lays
    /// the box out, so relabelling earlier would set a layer frame once
    /// here and again in [`Self::finalize_hints`]. Two frame changes in two
    /// transactions means the second restarts the first's animation, and the box
    /// jumps instead of growing smoothly.
    ///
    /// Comparing the label against the one already in place costs a `String` and
    /// no measurement, so a traversal that never widens lays nothing out again.
    pub(super) fn relabel_hints(&mut self) {
        for (i, hb) in self.hint_boxes.iter_mut().enumerate() {
            let label = self
                .config
                .hint_keys
                .label_for_index(i, Some(self.hint_width));
            if label != hb.label {
                hb.set_label(label, &self.overlay_frame, &self.config.theme);
            }
        }
    }

    /// Push the final positions and labels to the layers without clearing, so
    /// nothing flickers.
    pub(super) fn finalize_hints(&self) {
        self.hint_boxes.iter().for_each(|hb| {
            hb.refresh(0, &self.overlay_frame, &self.config.theme);
        })
    }

    /// Show or hide the hint boxes, refresh their text and positions, and return
    /// the indices of the ones left visible.
    pub(super) fn update_hints(&mut self) -> Vec<usize> {
        let mut visible_indices = vec![];
        if self.hint_boxes.is_empty() {
            return visible_indices;
        }
        let prefix_len = self.hint_prefix.len();
        let mut nothing_visible = true;
        let search_pattern = search_regex(&self.search_prefix);

        autoreleasepool(|_| {
            for (idx, hb) in self.hint_boxes.iter_mut().enumerate() {
                let is_selected_side = self.multi_selection.is_on
                    && self
                        .multi_selection
                        .one_side_idx
                        .is_some_and(|idx| idx == hb.idx);
                let matches_search_prefix = search_pattern
                    .as_ref()
                    .is_none_or(|p| self.search_targets.get(idx).is_some_and(|h| p.is_match(h)));
                let visible = !hb.disabled
                    && hb.label.starts_with(&self.hint_prefix)
                    && matches_search_prefix
                    && !is_selected_side;

                hb.set_visible(visible);

                if visible {
                    hb.refresh(prefix_len, &self.overlay_frame, &self.config.theme);
                    nothing_visible = false;
                    visible_indices.push(idx);
                }
            }

            if !self.is_searching && nothing_visible {
                self.notify(back_hint("Nothing matches, press "), log::Level::Warn);
            }
        });

        visible_indices
    }

    /// The menu for `key_prefix`: `head`, then a row per reachable item, with the
    /// keys aligned on the longest one.
    fn build_menu_message(
        &self,
        head: MenuString,
        builtin_menu_items: &[MenuItem],
        need_editor: bool,
        need_action: bool,
        need_workflow: bool,
        key_prefix: &str,
    ) -> MenuString {
        let prefix_len = key_prefix.chars().count();
        let mut max_key_len = 1;
        let mut menu_items = Vec::new();

        // Skip static single key menu items
        // when searching for multi-key actions
        for it in builtin_menu_items {
            if it.key.starts_with(key_prefix) {
                max_key_len = max_key_len.max(it.key.chars().count());
                menu_items.push((it.key, it.description));
            }
        }

        // Editor entry
        if need_editor
            && let Some(editor) = self.config.editor.as_ref()
            && editor.key.starts_with(key_prefix)
        {
            max_key_len = max_key_len.max(editor.key.chars().count());
            menu_items.push((&editor.key, &editor.display));
        }

        // TODO: refactor this if we introduce actions for elements other than text
        if need_action {
            for action in self.config.text_actions.iter() {
                if action.key.starts_with(key_prefix) {
                    max_key_len = max_key_len.max(action.key.chars().count());
                    menu_items.push((&action.key, &action.display));
                }
            }
        }

        // Workflows valid for current selected element
        if need_workflow {
            for workflow in self.config.workflows.iter() {
                if workflow.key.starts_with(key_prefix) && self.is_workflow_valid(workflow) {
                    max_key_len = max_key_len.max(workflow.key.chars().count());
                    menu_items.push((&workflow.key, &workflow.display));
                }
            }
        }

        if menu_items.is_empty() {
            return wrong_key_sequence();
        }

        let mut menu = head;
        for (key, display) in menu_items {
            menu.push_row(key, display, prefix_len, max_key_len);
        }

        menu
    }

    pub(super) fn draw_menu(&self, msg: &MenuString) {
        self.drawer.draw_menu(msg, &self.config.theme);
    }

    pub(super) fn draw_dashboard(&mut self, key_prefix: &str) {
        // NOTE: need `self.last_app_window_info` for `is_workflow_valid` check,
        // but shouldn't set `self.selected` yet
        if self.selected.is_none() {
            self.get_app_window_info();
        }

        let msg = self.build_menu_message(
            MenuString::from("Pick a Target:"),
            &DASH_BOARD_MENU_ITEMS,
            true,
            false,
            true,
            key_prefix,
        );

        self.draw_menu(&msg);
    }

    fn draw_image_action_menu(&self, key_prefix: &str) {
        let msg = self.build_menu_message(
            MenuString::from("Pick an Action for Image:"),
            &IMAGE_ACTION_MENU_ITEMS,
            false,
            false,
            true,
            key_prefix,
        );

        self.draw_menu(&msg);
    }

    fn draw_text_action_menu(&self, text: &str, key_prefix: &str) {
        let text = preview_line(text);
        let mut head = MenuString::from("Pick an Action for Text:\n\n");
        head.push_styled(&text, MenuStyle::Text).push("\n");
        let msg =
            self.build_menu_message(head, &TEXT_ACTION_MENU_ITEMS, true, true, true, key_prefix);

        self.draw_menu(&msg);
    }

    fn draw_scrolling_menu(&self, key_prefix: &str) {
        if !self.config.hide_scrolling_menu {
            let msg = self.build_menu_message(
                MenuString::from("Pick a Scrolling Action:"),
                &SCROLLBAR_MENU_ITEMS,
                false,
                false,
                false,
                key_prefix,
            );
            self.draw_menu(&msg);
        }
    }

    pub(super) fn draw_element_menu(
        &mut self,
        key_prefix: &str,
        role: RoleOfInterest,
        set_mode: bool,
    ) {
        // Set mode before drawing to make it more responsive
        if set_mode {
            match role {
                RoleOfInterest::Image => self.set_mode(Mode::ImageActionMenu),
                RoleOfInterest::ScrollBar => self.set_mode(Mode::Scrolling),
                RoleOfInterest::TextField
                | RoleOfInterest::StaticText
                | RoleOfInterest::PseudoText => self.set_mode(Mode::TextActionMenu),
                _ if self.target == Target::Text => self.set_mode(Mode::TextActionMenu),
                _ if self.target == Target::Scrollable => self.set_mode(Mode::Scrolling),
                _ => self.set_mode(Mode::DashBoard),
            }
        }

        let text_action_helper = || {
            let text = self
                .selected
                .as_ref()
                .and_then(|eoi| eoi.context.as_ref())
                .expect("Internal Error: selected text should be ready for text action menu");
            self.draw_text_action_menu(text, key_prefix);
        };

        match role {
            RoleOfInterest::Image => self.draw_image_action_menu(key_prefix),
            RoleOfInterest::ScrollBar => self.draw_scrolling_menu(key_prefix),
            RoleOfInterest::TextField | RoleOfInterest::StaticText | RoleOfInterest::PseudoText => {
                text_action_helper();
            }
            _ if self.target == Target::Text => text_action_helper(),
            _ if self.target == Target::Scrollable => self.draw_scrolling_menu(key_prefix),
            _ => self.draw_dashboard(key_prefix),
        }
    }

    pub(super) fn menu_refresh(&mut self, key_prefix: &str, set_mode: bool) {
        if let Some(eoi) = self.selected.as_ref() {
            self.draw_element_menu(key_prefix, eoi.role(), set_mode);
        }
    }

    pub(super) fn draw_word_picker(&mut self) {
        let word_picker = self
            .word_picker
            .as_mut()
            .expect("Internal Error: No word picker set.");

        word_picker.update_text_layer(
            &self.config.theme,
            &self.drawer,
            self.multi_selection.one_side_idx,
            &self.hint_prefix,
            &self.search_prefix,
        );
    }
}
