use super::{AppEngine, MAX_TEXT_DISPLAY_LEN, lifecycle, workflow};
use crate::{
    DASH_BOARD_MENU_ITEMS, IMAGE_ACTION_MENU_ITEMS, MenuItem, Mode, SCROLLBAR_MENU_ITEMS,
    TEXT_ACTION_MENU_ITEMS,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    drawer::GlyphlowDrawingLayer,
    util::{Frame, HintBox},
};
use objc2::rc::Retained;
use objc2_quartz_core::CALayer;

pub(crate) fn clear_drawing(executor: &AppEngine) {
    executor.window.clear();
}

pub(crate) fn draw_selected_frame(executor: &AppEngine) {
    if let Some(ElementOfInterest { frame, .. }) = executor.selected.as_ref() {
        executor.window.draw_frame_box(
            &frame.invert_y(executor.screen_size.height),
            &executor.config.theme.hint_bg_color,
        );
    }
}

pub(crate) fn draw_hints(executor: &AppEngine, boxes: &[HintBox]) {
    clear_drawing(executor);
    // NOTE: only select the other side of the same role,
    // and excluding the already selected one.
    if executor.multi_selection.is_on
        && let Some(one_idx) = executor.multi_selection.one_side_idex
    {
        let iter = boxes.iter().filter(|hb| hb.idx != one_idx);
        executor.window.draw_hints(
            iter,
            &executor.config.theme,
            executor.key_prefix.len(),
            executor.screen_size,
        );
    } else {
        executor.window.draw_hints(
            boxes.iter(),
            &executor.config.theme,
            executor.key_prefix.len(),
            executor.screen_size,
        );
    };
    draw_selected_frame(executor);
}

fn menu_format_helper(key: &str, display: &str, prefix_len: usize, max_key_len: usize) -> String {
    let padding = " ".repeat(max_key_len - key.chars().count());
    let filling = "_".repeat(prefix_len);
    format!(
        "\n{padding}({filling}{}) {display}",
        key.chars().skip(prefix_len).collect::<String>(),
    )
}

fn menu_msg_alignment_helper(
    executor: &AppEngine,
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
        && let Some(editor) = executor.config.editor.as_ref()
        && editor.key.starts_with(key_prefix)
    {
        max_key_len = max_key_len.max(editor.key.chars().count());
        menu_itmes.push((&editor.key, &editor.display));
    }

    // TODO: refactor this if we introduce actions for elements other than text
    if need_action {
        for action in executor.config.text_actions.iter() {
            if action.key.starts_with(key_prefix) {
                max_key_len = max_key_len.max(action.key.chars().count());
                menu_itmes.push((&action.key, &action.display));
            }
        }
    }

    // Workflows valid for current selected element
    if need_workflow {
        for workflow in executor.config.workflows.iter() {
            if workflow.key.starts_with(key_prefix)
                && workflow::is_workflow_valid(executor, workflow)
            {
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
        msg.push_str(&menu_format_helper(key, display, prefix_len, max_key_len));
    }

    msg
}

pub(crate) fn draw_menu(executor: &AppEngine, msg: &str) -> Retained<CALayer> {
    executor
        .window
        .draw_menu(msg, executor.screen_size, &executor.config.theme)
}

pub(crate) fn draw_dashboard(executor: &AppEngine, key_prefix: &str) {
    let msg = menu_msg_alignment_helper(
        executor,
        "Pick a Target:",
        &DASH_BOARD_MENU_ITEMS,
        true,
        false,
        true,
        key_prefix,
    );

    clear_drawing(executor);
    draw_selected_frame(executor);
    draw_menu(executor, &msg);
}

pub(crate) fn draw_image_action_menu(executor: &AppEngine, key_prefix: &str) {
    let msg = menu_msg_alignment_helper(
        executor,
        "Pick an Action for Image:",
        &IMAGE_ACTION_MENU_ITEMS,
        false,
        false,
        true,
        key_prefix,
    );

    draw_menu(executor, &msg);
}

pub(crate) fn draw_text_action_menu(executor: &AppEngine, text: &str, key_prefix: &str) {
    // Truncate long text
    let text = if text.len() > MAX_TEXT_DISPLAY_LEN {
        &format!("{:.max_len$}...", text, max_len = MAX_TEXT_DISPLAY_LEN)
    } else {
        text
    };
    let header = format!("Pick an Action for Text:\n\n{}\n", text);
    let msg = menu_msg_alignment_helper(
        executor,
        &header,
        &TEXT_ACTION_MENU_ITEMS,
        true,
        true,
        true,
        key_prefix,
    );

    draw_menu(executor, &msg);
}

pub(crate) fn draw_scrolling_menu(executor: &AppEngine, key_prefix: &str) {
    let msg = menu_msg_alignment_helper(
        executor,
        "Pick a Scrolling Action:",
        &SCROLLBAR_MENU_ITEMS,
        false,
        false,
        false,
        key_prefix,
    );
    draw_menu(executor, &msg);
}

pub(crate) fn draw_element_menu(
    executor: &AppEngine,
    key_prefix: &str,
    role: &RoleOfInterest,
    set_mode: bool,
) {
    clear_drawing(executor);
    // Set mode before drawing to make it more responsive
    if set_mode {
        match role {
            RoleOfInterest::Image => lifecycle::set_mode(executor, Mode::ImageActionMenu),
            RoleOfInterest::ScrollBar => lifecycle::set_mode(executor, Mode::Scrolling),
            RoleOfInterest::TextField | RoleOfInterest::StaticText | RoleOfInterest::PseudoText => {
                lifecycle::set_mode(executor, Mode::TextActionMenu)
            }
            _ if executor.target == Target::Text => {
                lifecycle::set_mode(executor, Mode::TextActionMenu)
            }
            _ if executor.target == Target::Scrollable => {
                lifecycle::set_mode(executor, Mode::Scrolling)
            }
            _ => lifecycle::set_mode(executor, Mode::DashBoard),
        }
    }

    let text_action_helper = || {
        let text = executor
            .selected
            .as_ref()
            .and_then(|eoi| eoi.context.as_ref())
            .expect("Internal Error: selected text should be ready for text action menu");
        draw_text_action_menu(executor, text, key_prefix);
    };

    match role {
        RoleOfInterest::Image => draw_image_action_menu(executor, key_prefix),
        RoleOfInterest::ScrollBar => draw_scrolling_menu(executor, key_prefix),
        RoleOfInterest::TextField | RoleOfInterest::StaticText | RoleOfInterest::PseudoText => {
            text_action_helper();
        }
        _ if executor.target == Target::Text => text_action_helper(),
        _ if executor.target == Target::Scrollable => draw_scrolling_menu(executor, key_prefix),
        _ => draw_dashboard(executor, key_prefix),
    }
}

pub(crate) fn menu_refresh(executor: &AppEngine, key_prefix: &str, set_mode: bool) {
    if let Some(eoi) = executor.selected.as_ref() {
        draw_element_menu(executor, key_prefix, &eoi.role, set_mode);
    } else {
        clear_drawing(executor);
        draw_dashboard(executor, key_prefix);
    }
}

pub(crate) fn draw_word_picker(executor: &mut AppEngine) {
    let word_picker = executor
        .word_picker
        .as_mut()
        .expect("Internal Error: No word picker set.");

    word_picker.update_text_layer(executor.multi_selection.one_side_idex);
}

pub(crate) fn draw_hints_from_cache(executor: &mut AppEngine) {
    let (hint_width, new_boxes) = executor.element_cache.hint_boxes(
        &Frame::from_origion(executor.screen_size),
        &executor.config.theme,
        executor.config.colored_frame_min_size as f64,
    );
    executor.hint_width = hint_width;
    executor.hint_boxes = new_boxes;
    draw_hints(executor, &executor.hint_boxes);
}
