use std::collections::HashSet;

use super::{AppEngine, drawing, interaction, lifecycle, workflow};
use crate::{
    FilterMode, Mode,
    action::perform_ocr,
    app_engine::filtering,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    util::{Frame, hint_boxes_from_frames, select_range_helper},
};
use log::Level;

pub(crate) fn ocr_res_filtering(executor: &mut AppEngine) {
    let ocr_res = executor
        .ocr_cache
        .as_ref()
        .expect("Internal Error: OCR cache not set.");
    let len = ocr_res.len();
    let iter = ocr_res.iter().map(|(_, rect)| Frame::from_cgrect(rect));
    let (digits, ocr_hints) = hint_boxes_from_frames(
        len,
        iter,
        &Frame::from_origion(executor.screen_size),
        &executor.config.theme,
        executor.config.colored_frame_min_size as f64,
    );
    executor.hint_width = digits;

    let filtered = ocr_hints
        .iter()
        .filter(|b| b.label.starts_with(&executor.key_prefix))
        .cloned()
        .collect::<Vec<_>>();

    if executor.key_prefix.len() == digits as usize
        && let Some(hb) = filtered.first()
    {
        if executor.multi_selection.is_on {
            if let Some((idx1, idx2)) = executor.multi_selection.set_one_side(hb.idx) {
                let choices: Vec<(String, Frame, bool)> = ocr_res
                    .iter()
                    .map(|(s, rect)| (s.clone(), Frame::from_cgrect(rect), true))
                    .collect::<Vec<_>>();
                let (text, frame) = select_range_helper(&choices, idx1, idx2)
                    .expect("Internal Error: wrong ocr hint indexing.");
                executor.selected = Some(ElementOfInterest::pseudo(None, frame));
                interaction::update_selected_text_and_show_menu(executor, text.clone());
            } else {
                executor.key_prefix.clear();
                drawing::draw_hints(executor, &ocr_hints);
            }
        } else {
            let (selected_text, cg_rect) = ocr_res
                .get(hb.idx)
                .expect("Internal Error: wrong ocr hint indexing.");
            // Context initialized as None, but updated right after
            executor.selected = Some(ElementOfInterest::pseudo(None, Frame::from_cgrect(cg_rect)));
            interaction::update_selected_text_and_show_menu(executor, selected_text.clone());
        }
    } else {
        drawing::draw_hints(executor, &filtered);
    }
}

pub(crate) async fn perform_ocr_on_frame(executor: &mut AppEngine, frame: Frame) {
    drawing::clear_drawing(executor);
    // NOTE: for images with parts out of sight
    let frame = frame
        .intersect(&Frame::from_origion(executor.screen_size))
        .unwrap_or(frame);
    match perform_ocr(&frame, &executor.config.ocr_languages).await {
        Ok(ocr_res) if !ocr_res.is_empty() => {
            executor.ocr_cache = Some(ocr_res);
            executor.key_prefix.clear();
            lifecycle::set_mode(executor, Mode::OCRResultFiltering);
            ocr_res_filtering(executor);
        }
        Err(e) => {
            lifecycle::notify_then_deactivate(
                executor,
                &format!("OCR failed: {e:?}"),
                Level::Error,
            );
        }
        _ => {
            lifecycle::notify_then_deactivate(executor, "Empty OCR result.", Level::Warn);
        }
    }
}

/// Filter the UI elements and redraw hints.
pub(crate) async fn filter_by_key(executor: &mut AppEngine) {
    let filtered_boxes = executor
        .hint_boxes
        .iter()
        .filter(|b| b.label.starts_with(&executor.key_prefix))
        .cloned()
        .collect::<Vec<_>>();

    // Only 1 remaining, take some actions
    if executor.key_prefix.len() == executor.hint_width as usize
        && filtered_boxes.len() == 1
        && let Some(crate::util::HintBox { idx, .. }) = filtered_boxes.first()
        && let Some(
            eoi @ ElementOfInterest {
                element: Some(element),
                context,
                frame,
                role,
                ..
            },
        ) = executor.element_cache.cache.get(*idx)
    {
        match executor.target {
            Target::MenuItem | Target::Clickable => {
                let center = frame.center();
                interaction::press_on_element(executor, element, role, center);
                lifecycle::deactivate(executor);
            }
            Target::Image => {
                executor.selected = Some(eoi.clone());
                drawing::draw_element_menu(executor, "", role, true);
            }
            Target::Custom(_) => {
                executor.selected = Some(eoi.clone());
                workflow::execute_pending_workflow_actions(executor);
            }
            Target::ImageOCR => perform_ocr_on_frame(executor, *frame).await,
            Target::Text => {
                if executor.multi_selection.is_on {
                    if let Some((idx1, idx2)) = executor.multi_selection.set_one_side(*idx) {
                        // NOTE: Role based filtering only when the roles on both sides match
                        let role_ref = if executor
                            .multi_selection
                            .role
                            .as_ref()
                            .is_some_and(|other| *role == *other)
                        {
                            Some(role)
                        } else {
                            None
                        };
                        let (text, frame) = executor
                            .element_cache
                            .select_range(idx1, idx2, role_ref)
                            .expect("Internal Error: wrong indexing of hints.");
                        executor.selected = Some(ElementOfInterest::pseudo(None, frame));
                        interaction::update_selected_text_and_show_menu(executor, text);
                    } else {
                        executor.multi_selection.role = Some(*role);
                        executor.key_prefix.clear();
                        drawing::draw_hints(executor, &executor.hint_boxes);
                    }
                } else if context.is_some() {
                    executor.selected = Some(eoi.clone());
                    drawing::draw_element_menu(executor, "", role, true);
                }
            }
            Target::ChildElement => {
                executor.selected = Some(eoi.clone());
                lifecycle::ui_element_traverse_on_activation(executor, Target::ChildElement);
                // Quick follow if only 1 element remaining
                // NOTE: use count to avoid circular pointer
                let mut count = 0;
                while executor.element_cache.cache.len() == 1 && count < 10 {
                    count += 1;
                    executor.selected = Some(executor.element_cache.cache[0].clone());
                    lifecycle::ui_element_traverse_on_activation(executor, Target::ChildElement);
                }

                // Actions for current selected element
                if executor.element_cache.cache.is_empty() {
                    let role = executor
                        .selected
                        .as_ref()
                        .map(|eoi| eoi.role)
                        .unwrap_or_default();
                    drawing::draw_element_menu(executor, "", &role, true);
                } else {
                    drawing::draw_hints_from_cache(executor);
                }
            }
            Target::Scrollable => {
                executor.selected = Some(eoi.clone());
                lifecycle::clear_cache(executor);
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, true);
            }
            Target::Editable => {
                executor.selected = Some(eoi.clone());
                interaction::focus_on_element(element);
                lifecycle::deactivate(executor);
            }
            Target::Edit => {
                executor.editing = Some(eoi.clone());
                // Focused before editing to increase the success rate
                interaction::focus_on_element(element);
                let text = context.clone().unwrap_or_default();
                match interaction::open_editor(executor, &text) {
                    Ok(_) => {
                        lifecycle::set_mode(executor, Mode::Editing);
                        executor.selected = None;
                    }
                    Err(e) => {
                        lifecycle::notify_then_deactivate(
                            executor,
                            &format!("Failed to open editor: {e}"),
                            Level::Error,
                        );
                    }
                }
                drawing::clear_drawing(executor);
            }
        }
    } else {
        drawing::draw_hints(executor, &filtered_boxes);
    }
}

pub(crate) async fn quick_follow(executor: &mut AppEngine) {
    if executor.element_cache.cache.len() == 1 {
        executor.key_prefix.push('A');
        filter_by_key(executor).await;
    }
}

pub(crate) async fn go_back_in_filtering(executor: &mut AppEngine, mode: FilterMode) {
    match mode {
        // Go back 1 level in element explorer
        FilterMode::Generic if executor.target == Target::ChildElement => {
            if interaction::select_parent(executor) {
                lifecycle::activate(executor, Target::ChildElement);
            }
        }
        FilterMode::WordPicking => {
            if let Some(wp) = executor.word_picker.as_mut()
                && wp.is_searching
            {
                wp.finish_searching(executor.multi_selection.one_side_idex);
                executor.key_prefix = wp.label_prefix.clone();
            } else if !executor.multi_selection.is_on
                || executor.multi_selection.one_side_idex.is_none()
            {
                // Go back to text action menu
                executor.word_picker = None;
                drawing::draw_element_menu(executor, "", &RoleOfInterest::PseudoText, true);
            } else {
                executor.multi_selection.clear_one_side();
                drawing::draw_word_picker(executor);
            }
        }
        FilterMode::Generic if executor.multi_selection.is_on => {
            executor.multi_selection.clear_one_side();
            filter_by_key(executor).await;
        }
        FilterMode::OCR if executor.multi_selection.is_on => {
            executor.multi_selection.clear_one_side();
            ocr_res_filtering(executor);
        }
        _ => (),
    }
}

pub(crate) async fn perform_filtering(executor: &mut AppEngine, key_char: char, mode: FilterMode) {
    if key_char == '-' {
        if executor.key_prefix.is_empty() {
            go_back_in_filtering(executor, mode).await;
            return;
        } else {
            executor.key_prefix.pop();
        }
    } else if executor.key_prefix.len() < executor.hint_width as usize
        || executor
            .word_picker
            .as_ref()
            .is_some_and(|wp| wp.is_searching)
    {
        executor.key_prefix.push(key_char);
    }

    match mode {
        FilterMode::OCR => {
            ocr_res_filtering(executor);
        }
        FilterMode::Generic => {
            filter_by_key(executor).await;
        }
        FilterMode::WordPicking => {
            if let Some(wp) = executor.word_picker.as_mut() {
                wp.update_prefix(&executor.key_prefix);
            };
            drawing::draw_word_picker(executor);
            filtering::check_word_picker(executor);
        }
    }
}

pub(crate) fn update_editing_text(executor: &mut AppEngine, new_text: String) {
    if let Some(crate::ax_element::ElementOfInterest {
        element: Some(ele), ..
    }) = executor.editing.as_ref()
    {
        use accessibility::AXUIElementAttributes;
        use core_foundation::{base::TCFType, string::CFString};
        if let Err(e) = ele.set_value(CFString::new(&new_text).as_CFType()) {
            log::warn!("Failed to set the text of focused element: {ele:?}\n Error: {e}");
            // Reset editing upon failure
            executor.editing = None;
        }
    }
}

/// If only 1 word is matched, then update the selected text and show the menu
pub(crate) fn check_word_picker(executor: &mut AppEngine) {
    let Some(wp) = executor.word_picker.as_mut() else {
        return;
    };
    let matched_words = wp.matched_words();
    let is_searching = wp.is_searching;

    // Duplicated words when multi_selection is off
    let unique_matching = matched_words.len() == 1
        || (!executor.multi_selection.is_on
            && matched_words
                .iter()
                .map(|(_, w)| w)
                .collect::<HashSet<_>>()
                .len()
                == 1);

    if !is_searching
        && (executor.key_prefix.len() == executor.hint_width as usize
            || (!wp.text_prefix.is_empty() && wp.label_prefix.is_empty()))
        && unique_matching
        && let Some((idx, text)) = matched_words.first()
    {
        if executor.multi_selection.is_on {
            if let Some((idx1, idx2)) = executor.multi_selection.set_one_side(*idx) {
                let text = executor
                    .word_picker
                    .as_ref()
                    .expect("Internal Error: no word picker set yet.")
                    .select_range(idx1, idx2)
                    .expect("Internal Error: wrong word picker indexing.");
                interaction::update_selected_text_and_show_menu(executor, text.clone())
            } else {
                executor.key_prefix.clear();
                // Reset for another side
                if let Some(wp) = executor.word_picker.as_mut() {
                    wp.text_prefix.clear();
                    wp.label_prefix.clear()
                };
                drawing::draw_word_picker(executor);
            }
        } else {
            interaction::update_selected_text_and_show_menu(executor, text.clone())
        }
    }
}

pub(crate) fn toggle_multiselection(executor: &mut AppEngine) {
    executor.multi_selection.toggle();
    let on_off = if executor.multi_selection.is_on {
        "on"
    } else {
        "off"
    };
    lifecycle::notify(
        executor,
        &format!("Multi-selection is now {on_off}."),
        Level::Info,
    );
}
