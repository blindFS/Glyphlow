use std::collections::HashSet;

use super::AppEngine;
use crate::{
    FilterMode, Mode,
    action::perform_ocr,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    user_interface::{HintBox, hint_boxes_from_frames},
    util::{Frame, select_range_helper},
};
use log::Level;

impl AppEngine {
    fn ocr_res_filtering(&mut self) {
        if self.hint_boxes.is_empty() {
            let (digits, ocr_hints) = {
                let ocr_res = self
                    .ocr_cache
                    .as_ref()
                    .expect("Internal Error: OCR cache not set.");
                let len = ocr_res.len();
                let iter = ocr_res.iter().map(|(_, rect)| Frame::from_cgrect(rect));
                hint_boxes_from_frames(
                    len,
                    iter,
                    &Frame::from_origion(self.screen_size),
                    &self.config.theme,
                    self.config.colored_frame_min_size as f64,
                )
            };
            self.hint_width = digits;
            self.hint_boxes = ocr_hints;
            self.draw_hints();
        } else {
            self.update_hints();
        }

        let filtered_idx = self
            .hint_boxes
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .map(|b| b.idx)
            .collect::<Vec<_>>();

        if self.key_prefix.len() == self.hint_width as usize
            && let Some(&hb_idx) = filtered_idx.first()
        {
            if self.multi_selection.is_on {
                if let Some((idx1, idx2)) = self.multi_selection.set_one_side(hb_idx) {
                    let choices: Vec<(String, Frame, bool)> = self
                        .ocr_cache
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|(s, rect)| (s.clone(), Frame::from_cgrect(rect), true))
                        .collect::<Vec<_>>();
                    let (text, frame) = select_range_helper(&choices, idx1, idx2)
                        .expect("Internal Error: wrong ocr hint indexing.");
                    self.select(ElementOfInterest::pseudo(None, frame));
                    self.clear_hints();
                    self.update_selected_text_and_show_menu(text.clone());
                } else {
                    self.key_prefix.clear();
                    self.update_hints();
                }
            } else {
                let (selected_text, cg_rect) = self
                    .ocr_cache
                    .as_ref()
                    .unwrap()
                    .get(hb_idx)
                    .expect("Internal Error: wrong ocr hint indexing.");
                let selected_text = selected_text.clone();
                let frame = Frame::from_cgrect(cg_rect);
                // Context initialized as None, but updated right after
                self.select(ElementOfInterest::pseudo(None, frame));
                self.clear_hints();
                self.update_selected_text_and_show_menu(selected_text);
            }
        }
    }

    pub(super) async fn perform_ocr_on_frame(&mut self, frame: Frame) {
        self.hint_boxes.clear();
        // NOTE: for images with parts out of sight
        let frame = frame
            .intersect(&Frame::from_origion(self.screen_size))
            .unwrap_or(frame);
        match perform_ocr(&frame, &self.config.ocr_languages).await {
            Ok(ocr_res) if !ocr_res.is_empty() => {
                self.ocr_cache = Some(ocr_res);
                self.key_prefix.clear();
                self.set_mode(Mode::OCRResultFiltering);
                self.ocr_res_filtering();
            }
            Err(e) => {
                self.notify_then_deactivate(&format!("OCR failed: {e:?}"), Level::Error);
            }
            _ => {
                self.notify_then_deactivate("Empty OCR result.", Level::Warn);
            }
        }
    }

    /// Filter the UI elements and redraw hints.
    async fn filter_by_key(&mut self) {
        let filtered_boxes = self
            .hint_boxes
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .cloned()
            .collect::<Vec<_>>();

        // Only 1 remaining, take some actions
        if self.key_prefix.len() == self.hint_width as usize
            && filtered_boxes.len() == 1
            && let Some(HintBox { idx, .. }) = filtered_boxes.first()
            && let Some(eoi) = self.element_cache.cache.get(*idx)
            && let Some(element) = eoi.element()
        {
            if !self.multi_selection.is_on {
                self.clear_hints();
            }

            let role = eoi.role();
            let context = &eoi.context;
            let frame = &eoi.frame;
            match self.target {
                Target::MenuItem | Target::Clickable => {
                    let center = frame.center();
                    self.press_on_element(element, &role, center);
                    self.deactivate();
                }
                Target::Image => {
                    self.select(eoi.clone());
                    self.draw_element_menu("", role, true);
                }
                Target::Custom(_) => {
                    self.select(eoi.clone());
                    self.execute_pending_workflow_actions();
                }
                Target::ImageOCR => self.perform_ocr_on_frame(*frame).await,
                Target::Text => {
                    if self.multi_selection.is_on {
                        if let Some((idx1, idx2)) = self.multi_selection.set_one_side(*idx) {
                            // NOTE: Role based filtering only when the roles on both sides match
                            let role_ref = if self
                                .multi_selection
                                .role
                                .as_ref()
                                .is_some_and(|other| role == *other)
                            {
                                Some(&role)
                            } else {
                                None
                            };
                            let (text, frame) = self
                                .element_cache
                                .select_range(idx1, idx2, role_ref)
                                .expect("Internal Error: wrong indexing of hints.");
                            self.select(ElementOfInterest::pseudo(None, frame));
                            self.clear_hints();
                            self.update_selected_text_and_show_menu(text);
                        } else {
                            self.multi_selection.role = Some(role);
                            self.key_prefix.clear();
                            self.update_hints();
                        }
                    } else if context.is_some() {
                        self.select(eoi.clone());
                        self.draw_element_menu("", role, true);
                    }
                }
                Target::ChildElement => {
                    self.select(eoi.clone());
                    self.activate(Target::ChildElement);
                    // Quick follow if only 1 element remaining
                    // NOTE: use count to avoid circular pointer
                    let mut count = 0;
                    while self.element_cache.cache.len() == 1 && count < 10 {
                        count += 1;
                        self.select(self.element_cache.cache[0].to_owned());
                        self.activate(Target::ChildElement);
                    }

                    // Actions for current selected element
                    if self.element_cache.cache.is_empty() {
                        let role = self
                            .selected
                            .as_ref()
                            .map(|eoi| eoi.role())
                            .unwrap_or_default();
                        self.draw_element_menu("", role, true);
                    }
                }
                Target::Scrollable => {
                    self.select(eoi.clone());
                    self.draw_element_menu("", RoleOfInterest::ScrollBar, true);
                }
                Target::Editable => {
                    self.selected = Some(eoi.clone());
                    self.focus_on_element(element);
                    self.deactivate();
                }
                Target::Edit => {
                    self.editing = Some(eoi.clone());
                    // Focused before editing to increase the success rate
                    self.focus_on_element(element);
                    let text = context.clone().unwrap_or_default();
                    match self.open_editor(&text) {
                        Ok(_) => {
                            self.set_mode(Mode::Editing);
                            self.selected = None;
                        }
                        Err(e) => {
                            self.notify_then_deactivate(
                                &format!("Failed to open editor: {e}"),
                                Level::Error,
                            );
                        }
                    }
                }
            }
        } else {
            self.update_hints();
        }
    }

    pub(super) async fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.key_prefix.push('A');
            self.filter_by_key().await;
        }
    }

    fn go_back_in_filtering(&mut self, mode: FilterMode) {
        match mode {
            // Go back 1 level in element explorer
            FilterMode::Generic if self.target == Target::ChildElement => {
                if self.select_parent() {
                    self.activate(Target::ChildElement);
                }
            }
            FilterMode::WordPicking => {
                if let Some(wp) = self.word_picker.as_mut()
                    && wp.is_searching
                {
                    wp.finish_searching(self.multi_selection.one_side_idex);
                    self.key_prefix = wp.label_prefix.clone();
                } else if !self.multi_selection.is_on
                    || self.multi_selection.one_side_idex.is_none()
                {
                    // Go back to text action menu
                    self.word_picker = None;
                    self.draw_element_menu("", RoleOfInterest::PseudoText, true);
                } else {
                    self.multi_selection.clear_one_side();
                    self.draw_word_picker();
                }
            }
            FilterMode::Generic if self.multi_selection.is_on => {
                self.multi_selection.clear_one_side();
                self.update_hints();
            }
            FilterMode::OCR if self.multi_selection.is_on => {
                self.multi_selection.clear_one_side();
                self.ocr_res_filtering();
            }
            _ => (),
        }
    }

    pub(super) async fn perform_filtering(&mut self, key_char: char, mode: FilterMode) {
        if key_char == '-' {
            if self.key_prefix.is_empty() {
                self.go_back_in_filtering(mode);
                return;
            } else {
                self.key_prefix.pop();
            }
        } else if self.key_prefix.len() < self.hint_width as usize
            || self.word_picker.as_ref().is_some_and(|wp| wp.is_searching)
        {
            self.key_prefix.push(key_char);
        }

        match mode {
            FilterMode::OCR => {
                self.ocr_res_filtering();
            }
            FilterMode::Generic => {
                self.filter_by_key().await;
            }
            FilterMode::WordPicking => {
                if let Some(wp) = self.word_picker.as_mut() {
                    wp.update_prefix(&self.key_prefix);
                };
                self.draw_word_picker();
                self.check_word_picker();
            }
        }
    }

    /// If only 1 word is matched, then update the selected text and show the menu
    pub(super) fn check_word_picker(&mut self) {
        let Some(wp) = self.word_picker.as_mut() else {
            return;
        };
        let matched_words = wp.matched_words();
        let is_searching = wp.is_searching;

        // Duplicated words when multi_selection is off
        let unique_matching = matched_words.len() == 1
            || (!self.multi_selection.is_on
                && matched_words
                    .iter()
                    .map(|(_, w)| w)
                    .collect::<HashSet<_>>()
                    .len()
                    == 1);

        if !is_searching
            && (self.key_prefix.len() == self.hint_width as usize
                || (!wp.text_prefix.is_empty() && wp.label_prefix.is_empty()))
            && unique_matching
            && let Some((idx, text)) = matched_words.first()
        {
            if self.multi_selection.is_on {
                if let Some((idx1, idx2)) = self.multi_selection.set_one_side(*idx) {
                    let text = self
                        .word_picker
                        .as_ref()
                        .expect("Internal Error: no word picker set yet.")
                        .select_range(idx1, idx2)
                        .expect("Internal Error: wrong word picker indexing.");
                    self.update_selected_text_and_show_menu(text.clone())
                } else {
                    self.key_prefix.clear();
                    // Reset for another side
                    if let Some(wp) = self.word_picker.as_mut() {
                        wp.text_prefix.clear();
                        wp.label_prefix.clear()
                    };
                    self.draw_word_picker();
                }
            } else {
                self.update_selected_text_and_show_menu(text.clone())
            }
        }
    }

    pub(super) fn toggle_multiselection(&mut self) {
        self.multi_selection.toggle();
        let on_off = if self.multi_selection.is_on {
            "on"
        } else {
            "off"
        };
        self.notify(&format!("Multi-selection is now {on_off}."), Level::Info);
    }
}
