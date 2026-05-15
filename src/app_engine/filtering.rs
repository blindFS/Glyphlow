use std::collections::HashSet;

use super::AppEngine;
use crate::{
    FilterMode, Mode,
    action::perform_ocr,
    ax_element::{ElementOfInterest, Target},
    config::RoleOfInterest,
    util::{Frame, hint_boxes_from_frames, select_range_helper},
};
use log::Level;

impl AppEngine {
    fn ocr_res_filtering(&mut self) {
        let ocr_res = self
            .ocr_cache
            .as_ref()
            .expect("Internal Error: OCR cache not set.");
        let len = ocr_res.len();
        let iter = ocr_res.iter().map(|(_, rect)| Frame::from_cgrect(rect));
        let (digits, ocr_hints) = hint_boxes_from_frames(
            len,
            iter,
            &Frame::from_origion(self.screen_size),
            &self.config.theme,
            self.config.colored_frame_min_size as f64,
        );
        self.hint_width = digits;

        let filtered = ocr_hints
            .iter()
            .filter(|b| b.label.starts_with(&self.key_prefix))
            .cloned()
            .collect::<Vec<_>>();

        if self.key_prefix.len() == digits as usize
            && let Some(hb) = filtered.first()
        {
            if self.multi_selection.is_on {
                if let Some((idx1, idx2)) = self.multi_selection.set_one_side(hb.idx) {
                    let choices: Vec<(String, Frame, bool)> = ocr_res
                        .iter()
                        .map(|(s, rect)| (s.clone(), Frame::from_cgrect(rect), true))
                        .collect::<Vec<_>>();
                    let (text, frame) = select_range_helper(&choices, idx1, idx2)
                        .expect("Internal Error: wrong ocr hint indexing.");
                    self.selected = Some(ElementOfInterest::pseudo(None, frame));
                    self.update_selected_text_and_show_menu(text.clone());
                } else {
                    self.key_prefix.clear();
                    self.draw_hints(&ocr_hints);
                }
            } else {
                let (selected_text, cg_rect) = ocr_res
                    .get(hb.idx)
                    .expect("Internal Error: wrong ocr hint indexing.");
                // Context initialized as None, but updated right after
                self.selected = Some(ElementOfInterest::pseudo(None, Frame::from_cgrect(cg_rect)));
                self.update_selected_text_and_show_menu(selected_text.clone());
            }
        } else {
            self.draw_hints(&filtered);
        }
    }

    pub(super) async fn perform_ocr_on_frame(&mut self, frame: Frame) {
        self.clear_drawing();
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
            && let Some(crate::util::HintBox { idx, .. }) = filtered_boxes.first()
            && let Some(
                eoi @ ElementOfInterest {
                    element: Some(element),
                    context,
                    frame,
                    role,
                    ..
                },
            ) = self.element_cache.cache.get(*idx)
        {
            match self.target {
                Target::MenuItem | Target::Clickable => {
                    let center = frame.center();
                    self.press_on_element(element, role, center);
                    self.deactivate();
                }
                Target::Image => {
                    self.selected = Some(eoi.clone());
                    self.draw_element_menu("", role, true);
                }
                Target::Custom(_) => {
                    self.selected = Some(eoi.clone());
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
                                .is_some_and(|other| *role == *other)
                            {
                                Some(role)
                            } else {
                                None
                            };
                            let (text, frame) = self
                                .element_cache
                                .select_range(idx1, idx2, role_ref)
                                .expect("Internal Error: wrong indexing of hints.");
                            self.selected = Some(ElementOfInterest::pseudo(None, frame));
                            self.update_selected_text_and_show_menu(text);
                        } else {
                            self.multi_selection.role = Some(*role);
                            self.key_prefix.clear();
                            self.draw_hints(&self.hint_boxes);
                        }
                    } else if context.is_some() {
                        self.selected = Some(eoi.clone());
                        self.draw_element_menu("", role, true);
                    }
                }
                Target::ChildElement => {
                    self.selected = Some(eoi.clone());
                    self.ui_element_traverse_on_activation(Target::ChildElement);
                    // Quick follow if only 1 element remaining
                    // NOTE: use count to avoid circular pointer
                    let mut count = 0;
                    while self.element_cache.cache.len() == 1 && count < 10 {
                        count += 1;
                        self.selected = Some(self.element_cache.cache[0].clone());
                        self.ui_element_traverse_on_activation(Target::ChildElement);
                    }

                    // Actions for current selected element
                    if self.element_cache.cache.is_empty() {
                        let role = self
                            .selected
                            .as_ref()
                            .map(|eoi| eoi.role)
                            .unwrap_or_default();
                        self.draw_element_menu("", &role, true);
                    } else {
                        self.draw_hints_from_cache();
                    }
                }
                Target::Scrollable => {
                    self.selected = Some(eoi.clone());
                    self.clear_cache();
                    self.draw_element_menu("", &RoleOfInterest::ScrollBar, true);
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
                    self.clear_drawing();
                }
            }
        } else {
            self.draw_hints(&filtered_boxes);
        }
    }

    pub(super) async fn quick_follow(&mut self) {
        if self.element_cache.cache.len() == 1 {
            self.key_prefix.push('A');
            self.filter_by_key().await;
        }
    }

    async fn go_back_in_filtering(&mut self, mode: FilterMode) {
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
                    self.draw_element_menu("", &RoleOfInterest::PseudoText, true);
                } else {
                    self.multi_selection.clear_one_side();
                    self.draw_word_picker();
                }
            }
            FilterMode::Generic if self.multi_selection.is_on => {
                self.multi_selection.clear_one_side();
                self.filter_by_key().await;
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
                self.go_back_in_filtering(mode).await;
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

    pub(super) fn update_editing_text(&mut self, new_text: String) {
        if let Some(crate::ax_element::ElementOfInterest {
            element: Some(ele), ..
        }) = self.editing.as_ref()
        {
            use accessibility::AXUIElementAttributes;
            use core_foundation::{base::TCFType, string::CFString};
            if let Err(e) = ele.set_value(CFString::new(&new_text).as_CFType()) {
                log::warn!("Failed to set the text of focused element: {ele:?}\n Error: {e}");
                // Reset editing upon failure
                self.editing = None;
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
