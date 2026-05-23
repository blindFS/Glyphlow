use super::AppEngine;
use crate::{
    DASH_BOARD_MENU_ITEMS, IMAGE_ACTION_MENU_ITEMS, MenuItem, Mode, SCROLLBAR_MENU_ITEMS,
    TEXT_ACTION_MENU_ITEMS,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    util::Frame,
};
use objc2::rc::autoreleasepool;

static MAX_TEXT_DISPLAY_LEN: usize = 30;

impl AppEngine {
    pub(super) fn clear_drawing(&mut self) {
        self.drawer.clear();
    }

    /// Clear hint drawings
    /// Should only be called on:
    /// 1. Deactivation
    /// 2. Only 1 matching remained while hint-based filtering
    pub(super) fn clear_hints(&self) {
        for hb in self.hint_boxes.iter() {
            hb.free();
        }
    }

    /// Change selected element of interest
    /// Draw/Update the frame box of selected element
    pub(super) fn select(&mut self, eoi: ElementOfInterest) {
        self.drawer
            .draw_frame(&eoi.frame.invert_y(self.screen_size.height));
        self.selected = Some(eoi);
    }

    pub(super) fn draw_frame_instant(&mut self, frame: &Frame) {
        self.drawer
            .draw_frame_instant(&frame.invert_y(self.screen_size.height));
    }

    /// Draw/Update hint boxes
    pub(super) fn draw_hints(&mut self) {
        autoreleasepool(|_| {
            for hb in self.hint_boxes.iter_mut() {
                hb.draw(
                    &self.drawer.root,
                    &self.config.theme,
                    self.key_prefix.len(),
                    self.screen_size,
                );
            }
        })
    }

    /// Show/Hide hint_boxes/colored_frames, update hint text and positions
    pub(super) fn update_hints(&mut self) {
        let prefix_len = self.key_prefix.len();

        autoreleasepool(|_| {
            for hb in self.hint_boxes.iter_mut() {
                let visible = hb.label.starts_with(&self.key_prefix)
                    && !(self.multi_selection.is_on
                        && self
                            .multi_selection
                            .one_side_idex
                            .is_some_and(|idx| idx == hb.idx));

                hb.set_visible(visible);

                if visible {
                    hb.refresh(prefix_len, self.screen_size, &self.config.theme);
                }
            }
        })
    }

    fn menu_format_helper(
        key: &str,
        display: &str,
        prefix_len: usize,
        max_key_len: usize,
    ) -> String {
        let padding = " ".repeat(max_key_len - key.chars().count());
        let filling = "_".repeat(prefix_len);
        format!(
            "\n{padding}({filling}{}) {display}",
            key.chars().skip(prefix_len).collect::<String>(),
        )
    }

    fn menu_msg_alignment_helper(
        &self,
        head: &str,
        builtin_menu_items: &[MenuItem],
        need_editor: bool,
        need_action: bool,
        need_workflow: bool,
        key_prefix: &str,
    ) -> String {
        let prefix_len = key_prefix.chars().count();
        let mut max_key_len = 1;
        let mut menu_itmes = Vec::new();

        // Skip static single key menu items
        // when searching for multi-key actions
        for it in builtin_menu_items {
            if it.key.starts_with(key_prefix) {
                max_key_len = max_key_len.max(it.key.chars().count());
                menu_itmes.push((it.key, it.description));
            }
        }

        // Editor entry
        if need_editor
            && let Some(editor) = self.config.editor.as_ref()
            && editor.key.starts_with(key_prefix)
        {
            max_key_len = max_key_len.max(editor.key.chars().count());
            menu_itmes.push((&editor.key, &editor.display));
        }

        // TODO: refactor this if we introduce actions for elements other than text
        if need_action {
            for action in self.config.text_actions.iter() {
                if action.key.starts_with(key_prefix) {
                    max_key_len = max_key_len.max(action.key.chars().count());
                    menu_itmes.push((&action.key, &action.display));
                }
            }
        }

        // Workflows valid for current selected element
        if need_workflow {
            for workflow in self.config.workflows.iter() {
                if workflow.key.starts_with(key_prefix) && self.is_workflow_valid(workflow) {
                    max_key_len = max_key_len.max(workflow.key.chars().count());
                    menu_itmes.push((&workflow.key, &workflow.display));
                }
            }
        }

        if menu_itmes.is_empty() {
            return "Wrong key sequence\nPress Backspace to go back".to_string();
        }

        // Aligned
        let mut msg = head.to_string();
        for (key, display) in menu_itmes {
            msg.push_str(&Self::menu_format_helper(
                key,
                display,
                prefix_len,
                max_key_len,
            ));
        }

        msg
    }

    fn draw_dashboard(&self, key_prefix: &str) {
        let msg = self.menu_msg_alignment_helper(
            "Pick a Target:",
            &DASH_BOARD_MENU_ITEMS,
            true,
            false,
            true,
            key_prefix,
        );

        self.drawer.draw_menu(&msg);
    }

    fn draw_image_action_menu(&self, key_prefix: &str) {
        let msg = self.menu_msg_alignment_helper(
            "Pick an Action for Image:",
            &IMAGE_ACTION_MENU_ITEMS,
            false,
            false,
            true,
            key_prefix,
        );

        self.drawer.draw_menu(&msg);
    }

    fn draw_text_action_menu(&self, text: &str, key_prefix: &str) {
        // Truncate long text
        let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
            &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
        } else {
            text
        };
        let header = format!("Pick an Action for Text:\n\n{}\n", text);
        let msg = self.menu_msg_alignment_helper(
            &header,
            &TEXT_ACTION_MENU_ITEMS,
            true,
            true,
            true,
            key_prefix,
        );

        self.drawer.draw_menu(&msg);
    }

    fn draw_scrolling_menu(&self, key_prefix: &str) {
        if !self.config.hide_scrolling_menu {
            let msg = self.menu_msg_alignment_helper(
                "Pick a Scrolling Action:",
                &SCROLLBAR_MENU_ITEMS,
                false,
                false,
                false,
                key_prefix,
            );
            self.drawer.draw_menu(&msg);
        }
    }

    pub(super) fn draw_element_menu(&self, key_prefix: &str, role: RoleOfInterest, set_mode: bool) {
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

    pub(super) fn menu_refresh(&self, key_prefix: &str, set_mode: bool) {
        if let Some(eoi) = self.selected.as_ref() {
            self.draw_element_menu(key_prefix, eoi.role(), set_mode);
        } else {
            self.draw_dashboard(key_prefix);
        }
    }

    pub(super) fn draw_word_picker(&mut self) {
        let word_picker = self
            .word_picker
            .as_mut()
            .expect("Internal Error: No word picker set.");

        word_picker.update_text_layer(&self.drawer, self.multi_selection.one_side_idex);
    }
}
