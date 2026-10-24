use super::{AppEngine, menu::MenuRow};
use crate::{
    MenuScope, Mode,
    ax_element::ElementOfInterest,
    user_interface::{MenuString, MenuStyle},
    util::search_regex,
};
use objc2::rc::autoreleasepool;
use std::borrow::Cow;

const MAX_TEXT_DISPLAY_LEN: usize = 30;

/// The selection as one line of at most [`MAX_TEXT_DISPLAY_LEN`] characters plus an
/// ellipsis.
fn preview_line(text: &str) -> Cow<'_, str> {
    if let Some((cut, _)) = text.char_indices().nth(MAX_TEXT_DISPLAY_LEN) {
        Cow::Owned(format!("{}...", &text[..cut]))
    } else {
        Cow::Borrowed(text)
    }
}

const BACK_KEY: &str = "󰁮";

/// The message for a key sequence that reaches no item: it says so, then offers
/// the way out.
pub(super) fn wrong_key_menu_string() -> MenuString {
    let mut msg = MenuString::styled("Wrong key sequence", MenuStyle::Header);
    msg.push("\nPress ")
        .push_styled(BACK_KEY, MenuStyle::Key)
        .push(" to go back");
    msg
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
    /// Must run *after* collision resolution: `HintBox::set_label` re-lays the
    /// box out, so relabelling earlier would move the layer twice and restart
    /// its animation.
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
                let mut msg = MenuString::styled("Nothing matches, press ", MenuStyle::Header);
                msg.push_styled(BACK_KEY, MenuStyle::Key)
                    .push_styled(" to go back", MenuStyle::Header);
                self.notify_styled(msg, log::Level::Warn);
            }
        });

        visible_indices
    }

    /// The menu for `scope` around `prefix`, laid out from the rows it reaches.
    ///
    /// `None` when it reaches no row at all, which is the caller's cue that the
    /// key reached nothing.
    fn menu_string<'a>(
        &self,
        scope: MenuScope,
        prefix: &str,
        rows: impl Iterator<Item = MenuRow<'a>> + Clone,
    ) -> Option<MenuString> {
        // The keys are aligned on the widest one, so the widest has to be known
        // before the first row is written: the rows are read twice rather than
        // collected. A duplicate key cannot widen the column, so only the drawing
        // pass drops duplicates.
        let key_width = rows.clone().map(|row| row.key.chars().count()).max()?;

        let mut msg = MenuString::styled(scope.header(), MenuStyle::Header);
        if scope == MenuScope::Text
            && let Some(text) = self.selected.as_ref().and_then(|eoi| eoi.context.as_ref())
        {
            msg.push("\n\n")
                .push_styled(&preview_line(text), MenuStyle::Text)
                .push("\n");
        }

        let prefix_len = prefix.chars().count();
        let mut claimed: Vec<&str> = Vec::new();
        for row in rows.filter(|row| {
            if claimed.contains(&row.key) {
                log::debug!("Menu key `{}` of `{}` is shadowed", row.key, row.display);
                false
            } else {
                claimed.push(row.key);
                true
            }
        }) {
            msg.push_key_map_entry(row.key, row.display, prefix_len, key_width);
        }
        Some(msg)
    }

    pub(super) fn draw_menu(&self, msg: &MenuString) {
        self.drawer.draw_menu(msg, &self.config.theme);
    }

    /// Draw `scope`'s menu around `prefix` and make it the current mode.
    pub(super) fn show_menu(&self, scope: MenuScope, prefix: &str) {
        self.show_menu_rows(scope, prefix, self.menu_rows(scope, prefix));
    }

    /// [`Self::show_menu`] over rows the caller already has.
    ///
    /// Returns whether `prefix` reaches any row, which is what decides if the key
    /// that produced it is worth keeping.
    pub(super) fn show_menu_rows<'a>(
        &self,
        scope: MenuScope,
        prefix: &str,
        rows: impl Iterator<Item = MenuRow<'a>> + Clone,
    ) -> bool {
        self.set_mode(Mode::Menu(scope));
        let msg = self.menu_string(scope, prefix, rows);
        let reachable = msg.is_some();
        if self.menu_visible(scope) {
            self.draw_menu(&msg.unwrap_or_else(wrong_key_menu_string));
        }
        reachable
    }

    /// Draw the dashboard, refreshing the focused window first: the workflows it
    /// offers are filtered by app.
    pub(super) fn draw_dashboard(&mut self) {
        if self.selected.is_none() {
            self.get_app_window_info();
        }
        self.show_menu(MenuScope::Dashboard, "");
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
