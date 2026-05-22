use super::AppEngine;
use crate::action::{WordPicker, get_dictionary_attributed_string};
use crate::ax_element::{ElementOfInterest, GetAttribute, SetAttribute};
use crate::config::RoleOfInterest;
use crate::util::Frame;
use crate::{Mode, ScrollAction, TextAction};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::{
    kAXErrorAttributeUnsupported, kAXErrorCannotComplete, kAXFocusedAttribute,
};
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber};
use log::Level;
use rdev::{Button, EventType, simulate};
use std::time::Duration;

impl AppEngine {
    pub(super) fn simulate_event(&self, event_type: &EventType) {
        match simulate(event_type) {
            Ok(()) => (),
            Err(e) => {
                log::error!("Failed to simulate event {event_type:?}: {e}");
            }
        }
    }

    pub(super) fn simulate_click(&self, x: f64, y: f64, button: Button) {
        self.simulate_event(&EventType::MouseMove { x, y });
        std::thread::sleep(Duration::from_millis(20));
        self.simulate_event(&EventType::ButtonPress(button));
        std::thread::sleep(Duration::from_millis(20));
        self.simulate_event(&EventType::ButtonRelease(button));
    }

    pub(super) fn focus_on_element(&self, element: &AXUIElement) {
        element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
    }

    pub(super) fn press_on_element(
        &self,
        element: &AXUIElement,
        role: &RoleOfInterest,
        center: (f64, f64),
    ) {
        let (x, y) = center;
        self.focus_on_element(element);

        if self.is_electron || *role == RoleOfInterest::Cell {
            self.simulate_click(x, y, Button::Left);
        } else if let Err(e) = element.press() {
            log::warn!("Failed to do UI press on element: {e}");
            match e {
                // NOTE: Sometimes this error is false alarm, usually because it takes longer
                // than expected, we shouldn't click in this case, otherwise it is performed twice.
                accessibility::Error::Ax(err_num)
                    if err_num == kAXErrorCannotComplete
                        || err_num == kAXErrorAttributeUnsupported => {}
                _ => {
                    log::info!("Simulating mouse click instead...");
                    self.simulate_click(x, y, Button::Left);
                }
            }
        };
    }

    fn scroll_to_value(&self, element: &AXUIElement, val: f64) {
        if let Err(e) = element.set_value(CFNumber::from(val.clamp(0.0, 1.0)).as_CFType()) {
            log::warn!("Failed to set value to the selected scroll bar: {e}.");
        };
    }

    pub(super) fn right_click_menu_on_element(&self, element: &AXUIElement, center: (f64, f64)) {
        let (x, y) = center;

        if self.is_electron {
            self.simulate_click(x, y, Button::Right);
        } else if let Err(e) = element.show_menu() {
            log::warn!("Failed to show menu on element: {e}");
            match e {
                // NOTE: Sometimes this error is false alarm, usually because it takes longer
                // than expected, we shouldn't click in this case, otherwise it is performed twice.
                accessibility::Error::Ax(err_num)
                    if err_num == kAXErrorCannotComplete
                        || err_num == kAXErrorAttributeUnsupported => {}
                _ => {
                    log::info!("Simulating mouse click instead...");
                    self.simulate_click(x, y, Button::Right);
                }
            }
        };
    }

    /// Select the parent of the currently selected element
    pub(super) fn select_parent(&mut self) -> bool {
        if let Some(parent_element) = self
            .selected
            .as_ref()
            .and_then(|eoi| eoi.element())
            .and_then(|ele| ele.parent().ok())
        {
            let screen_frame = Frame::from_origion(self.screen_size);
            let frame = parent_element.get_frame(screen_frame);
            self.select(ElementOfInterest::new(
                parent_element,
                None,
                RoleOfInterest::Generic,
                frame,
            ));
            return true;
        }
        false
    }

    pub(super) fn perform_text_action(&mut self, ta: TextAction) {
        let Some(ElementOfInterest {
            context: Some(text),
            ..
        }) = self.selected.as_ref()
        else {
            panic!("Internal Error: No selected text in Mode::TextActionMenu.");
        };

        let text = text.clone();

        // TODO:
        // 1. URL handling
        let keep_drawing = match ta {
            TextAction::Copy => {
                crate::action::text_to_clipboard(&text);
                self.notify_then_deactivate("Copied to clipboard.", Level::Info);
                true
            }
            TextAction::Dictionary => {
                log::trace!("Looking up `{text}` in Apple Dictionary.");
                if let Some(attr_string) = get_dictionary_attributed_string(
                    &text,
                    &self.config.dictionaries,
                    &self.config.theme,
                ) {
                    self.drawer.draw_attributed_string(attr_string, false);
                } else {
                    self.notify_then_deactivate("No definition found.", Level::Warn);
                }
                true
            }
            TextAction::Split => {
                self.set_mode(Mode::WordPicking);
                self.clear_cache();
                let word_picker = WordPicker::new(
                    text,
                    self.screen_size,
                    self.config.theme.clone(),
                    &self.drawer,
                );
                self.hint_width = word_picker.digits;

                self.word_picker = Some(word_picker);
                true
            }
            TextAction::Editor => {
                if let Err(e) = self.open_editor(&text) {
                    self.notify_then_deactivate(
                        &format!("Failed to open editor: {e}"),
                        Level::Error,
                    );
                    true
                } else {
                    false
                }
            }
            TextAction::UserDefined(idx) => {
                self.take_external_action(idx, &text);
                true
            }
        };

        if !keep_drawing {
            self.deactivate();
        }
    }

    pub(super) fn perform_scroll_action(&mut self, sa: ScrollAction) {
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(element) = selected.element() else {
            return;
        };
        let role = selected.role();
        let frame = selected.frame;

        if role == RoleOfInterest::ScrollBar {
            let Some(old_val) = element
                .value()
                .ok()
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|f| f.to_f64())
            else {
                self.deactivate();
                return;
            };

            let scroll_unit = self.config.scroll_distance;
            match sa {
                ScrollAction::DownRight => {
                    self.scroll_to_value(element, old_val + scroll_unit);
                }
                ScrollAction::UpLeft => {
                    self.scroll_to_value(element, old_val - scroll_unit);
                }
                ScrollAction::IncreaseDistance => {
                    self.config.scroll_distance *= 1.5;
                }
                ScrollAction::DecreaseDistance => {
                    self.config.scroll_distance /= 1.5;
                }
                ScrollAction::Top => {
                    self.scroll_to_value(element, 0.0);
                    self.draw_element_menu("", RoleOfInterest::ScrollBar, false);
                }
                ScrollAction::Bottom => {
                    self.scroll_to_value(element, 1.0);
                    self.draw_element_menu("", RoleOfInterest::ScrollBar, false);
                }
            }
        } else {
            let distance = (frame.size().1 * self.config.scroll_distance).max(1.0) as i64;
            match sa {
                ScrollAction::DownRight => {
                    self.simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: -distance,
                    });
                }
                ScrollAction::UpLeft => {
                    self.simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: distance,
                    });
                }
                ScrollAction::IncreaseDistance => {
                    self.config.scroll_distance *= 1.5;
                }
                ScrollAction::DecreaseDistance => {
                    self.config.scroll_distance /= 1.5;
                }
                ScrollAction::Top => {
                    self.simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: 999999,
                    });
                    self.draw_element_menu("", RoleOfInterest::ScrollBar, false);
                }
                ScrollAction::Bottom => {
                    self.simulate_event(&EventType::Wheel {
                        delta_x: 0,
                        delta_y: -999999,
                    });
                    self.draw_element_menu("", RoleOfInterest::ScrollBar, false);
                }
            }
        }
    }

    fn update_selected_text(&mut self, new_text: String) {
        if let Some(ElementOfInterest { context, .. }) = self.selected.as_mut() {
            *context = Some(new_text);
        }
    }

    pub(super) fn update_selected_text_and_show_menu(&mut self, new_text: String) {
        self.update_selected_text(new_text);
        self.draw_element_menu("", RoleOfInterest::PseudoText, true);
    }

    pub(super) fn update_editing_text(&mut self, new_text: String) {
        if let Some(selected) = self.editing.as_ref()
            && let Some(ele) = selected.element()
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

    pub(super) fn open_editor(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let editor = self
            .config
            .editor
            .as_ref()
            .expect("Internal Error: No editor set.");

        // Write current selected text to temp file
        let _ = std::fs::write(&self.temp_file, text);
        let temp_fp = self
            .temp_file
            .to_str()
            .unwrap_or_else(|| panic!("Failed to get temp file path for {:?}.", self.temp_file));

        let args = editor
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_temp_file}", temp_fp));
        let mut child = std::process::Command::new(&editor.command)
            .args(args)
            .spawn()?;

        std::thread::spawn(move || {
            if let Err(e) = child.wait() {
                log::error!("Editor failed to run: {e}");
            }
        });
        Ok(())
    }

    fn take_external_action(&mut self, idx: usize, selected_text: &str) {
        let action = self
            .config
            .text_actions
            .get(idx)
            .expect("Internal Error: text action index out of bounds.");
        let args = action
            .args
            .iter()
            .map(|arg| arg.replace("{glyphlow_text}", selected_text));
        let Ok(child) = std::process::Command::new(&action.command)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            self.notify_then_deactivate(
                &format!(
                    "Failed to spawn command: {} {}",
                    action.command,
                    action.args.join(" ")
                ),
                Level::Error,
            );
            return;
        };

        // Wait for the stdout as the new text
        match child.wait_with_output() {
            Ok(o) => {
                if !o.stdout.is_empty() {
                    let new_text = String::from_utf8_lossy(&o.stdout)
                        .trim_end_matches('\n')
                        .to_string();
                    self.update_selected_text_and_show_menu(new_text);
                } else if !o.stderr.is_empty() {
                    self.notify_then_deactivate(
                        &format!("External stderr: {}", String::from_utf8_lossy(&o.stderr)),
                        Level::Error,
                    );
                } else {
                    // Normal exit without new context
                    self.deactivate();
                }
            }
            Err(e) => {
                self.notify_then_deactivate(&format!("Failed to run command: {e}"), Level::Error);
            }
        }
    }
}
