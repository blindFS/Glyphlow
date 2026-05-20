use super::AppEngine;
use crate::{
    DASH_BOARD_MENU_ITEMS, IMAGE_ACTION_MENU_ITEMS, MenuItem, Mode, SCROLLBAR_MENU_ITEMS,
    TEXT_ACTION_MENU_ITEMS,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    drawer::{GlyphlowDrawingLayer, update_hint_text},
};
use objc2::rc::Retained;
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction};

static MAX_TEXT_DISPLAY_LEN: usize = 30;

impl AppEngine {
    pub(super) fn clear_drawing(&self) {
        self.window.clear();
    }

    fn draw_selected_frame(&self) {
        if let Some(ElementOfInterest { frame, .. }) = self.selected.as_ref() {
            self.window.draw_frame_box(
                &frame.invert_y(self.screen_size.height),
                &self.config.theme.hint_bg_color,
            );
        }
    }

    /// Draw/Update hint boxes
    pub(super) fn draw_hints(&mut self, incremental: bool) {
        log::log!(
            log::Level::Debug,
            "Start drawing hints, incremental: {incremental}"
        );
        if !incremental {
            self.clear_drawing();
            self.window.draw_hints(
                &mut self.hint_boxes,
                &self.config.theme,
                self.key_prefix.len(),
                self.screen_size,
            );
            self.draw_selected_frame();
        } else {
            self.update_hints();
        }
        log::log!(log::Level::Debug, "Finish drawing hints");
    }

    /// Show/Hide hint_boxes/colored_frames, update hint text and positions
    fn update_hints(&mut self) {
        let prefix_len = self.key_prefix.len();
        let sublayers = unsafe { self.window.sublayers().unwrap_or_default() };

        // NOTE: Safety check: if layers were cleared or out of sync, do nothing.
        // Happens when an immediate filtering key pressed right after the activation signal.
        if sublayers.count() < 2 {
            return;
        }

        let frames_root: Retained<CALayer> = sublayers.objectAtIndex(0);
        let boxes_root: Retained<CALayer> = sublayers.objectAtIndex(1);

        let frame_layers = unsafe { frames_root.sublayers().unwrap_or_default() };
        let box_layers = unsafe { boxes_root.sublayers().unwrap_or_default() };

        if frame_layers.count() < self.hint_boxes.len()
            || box_layers.count() < self.hint_boxes.len()
        {
            return;
        }

        let font_size = &self.config.theme.hint_font.pointSize();
        let tri_height = font_size / 2.0;
        let tri_width = font_size / 2.0;

        CATransaction::begin();
        for (i, hb) in self.hint_boxes.iter_mut().enumerate() {
            let frame_layer: Retained<CALayer> = frame_layers.objectAtIndex(i);
            let box_layer: Retained<CALayer> = box_layers.objectAtIndex(i);

            let visible = hb.label.starts_with(&self.key_prefix)
                && !(self.multi_selection.is_on
                    && self
                        .multi_selection
                        .one_side_idex
                        .is_some_and(|idx| idx == hb.idx));

            frame_layer.setHidden(!visible);
            box_layer.setHidden(!visible);

            if visible {
                if let Some(text_layer) = &hb.text_layer {
                    update_hint_text(text_layer, &hb.label, prefix_len, &self.config.theme);
                }

                // Update positions for collision resolution
                let box_size = box_layer.bounds().size;
                let box_width = box_size.width;
                let box_height = box_size.height;

                let (o_x, o_y) = (hb.x - box_width / 2.0, hb.y - tri_height - box_height);
                let o_x_move = o_x.min(self.screen_size.width - box_width).max(0.0);
                let o_y_move = o_y.max(0.0).min(self.screen_size.height - box_height);

                let origin = NSPoint::new(o_x_move, o_y_move);
                box_layer.setFrame(NSRect::new(origin, box_size));

                // Update triangle
                if let Some(tri_sublayers) = unsafe { box_layer.sublayers() }
                    && tri_sublayers.count() > 0
                {
                    let tri_layer: Retained<CALayer> = tri_sublayers.objectAtIndex(0);
                    let tri_x_offset = (box_width - tri_width) / 2.0;
                    let tri_y_offset = box_height;

                    let mut tri_frame = tri_layer.frame();
                    tri_frame.origin.x = tri_x_offset;
                    tri_frame.origin.y = tri_y_offset;
                    tri_layer.setFrame(tri_frame);

                    // Re-path the triangle to point to exact hint center
                    if let Ok(tri_shape_layer) = tri_layer.downcast::<CAShapeLayer>() {
                        let path = CGMutablePath::new();
                        let delta_x = hb.delta.0;
                        let delta_y = hb.delta.1;
                        let x_offset = o_x - o_x_move;
                        let y_offset = o_y - o_y_move;

                        unsafe {
                            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, 0.0);
                            CGMutablePath::add_line_to_point(
                                Some(&path),
                                std::ptr::null(),
                                tri_width / 2.0 - delta_x + x_offset,
                                tri_height - delta_y + y_offset,
                            );
                            CGMutablePath::add_line_to_point(
                                Some(&path),
                                std::ptr::null(),
                                tri_width,
                                0.0,
                            );
                        }
                        CGMutablePath::close_subpath(Some(&path));
                        tri_shape_layer.setPath(Some(&path));
                    }
                }
            }
        }
        CATransaction::commit();
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

    pub(super) fn draw_menu(&self, msg: &str) -> Retained<CALayer> {
        self.window
            .draw_menu(msg, self.screen_size, &self.config.theme)
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

        self.clear_drawing();
        self.draw_selected_frame();
        self.draw_menu(&msg);
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

        self.draw_menu(&msg);
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

        self.draw_menu(&msg);
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
            self.draw_menu(&msg);
        }
        self.draw_selected_frame();
    }

    pub(super) fn draw_element_menu(&self, key_prefix: &str, role: RoleOfInterest, set_mode: bool) {
        self.clear_drawing();
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
            self.clear_drawing();
            self.draw_dashboard(key_prefix);
        }
    }

    pub(super) fn draw_word_picker(&mut self) {
        let word_picker = self
            .word_picker
            .as_mut()
            .expect("Internal Error: No word picker set.");

        word_picker.update_text_layer(self.multi_selection.one_side_idex);
    }
}
