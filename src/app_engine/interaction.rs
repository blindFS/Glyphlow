use super::AppEngine;
use crate::action::{WordPicker, get_dictionary_attributed_string};
use crate::app_engine::lifecycle;
use crate::ax_element::{ElementOfInterest, GetAttribute};
use crate::config::RoleOfInterest;
use crate::drawer::GlyphlowDrawingLayer;
use crate::util::Frame;
use crate::{Mode, ScrollAction, TextAction};
use crate::{app_engine::drawing, ax_element::SetAttribute};
use accessibility::{AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::{
    kAXErrorAttributeUnsupported, kAXErrorCannotComplete, kAXFocusedAttribute,
};
use core_foundation::{base::TCFType, boolean::CFBoolean, number::CFNumber};
use log::Level;
use objc2_core_foundation::CGSize;
use rdev::{Button, EventType, simulate};
use std::time::Duration;

pub(crate) fn simulate_event(event_type: &EventType) {
    match simulate(event_type) {
        Ok(()) => (),
        Err(e) => {
            log::error!("Failed to simulate event {event_type:?}: {e}");
        }
    }
}

pub(crate) fn simulate_click(x: f64, y: f64, right: bool) {
    let button = if right { Button::Right } else { Button::Left };
    simulate_event(&EventType::MouseMove { x, y });
    std::thread::sleep(Duration::from_millis(20));
    simulate_event(&EventType::ButtonPress(button));
    std::thread::sleep(Duration::from_millis(20));
    simulate_event(&EventType::ButtonRelease(button));
}

pub(crate) fn focus_on_element(element: &AXUIElement) {
    element.set_attribute_by_name(kAXFocusedAttribute, CFBoolean::true_value().as_CFType());
}

pub(crate) fn press_on_element(
    executor: &AppEngine,
    element: &AXUIElement,
    role: &RoleOfInterest,
    center: (f64, f64),
) {
    let (x, y) = center;
    focus_on_element(element);

    if executor.is_electron || *role == RoleOfInterest::Cell {
        simulate_click(x, y, false);
    } else if let Err(e) = element.press() {
        log::warn!("Failed to do UI press on element: {e}");
        match e {
            // NOTE: Sometimes this error is false alarm, usually because it takes longer
            // than expected, we shouldn't click in this case, otherwise it is performed twice.
            accessibility::Error::Ax(err_num)
                if err_num == kAXErrorCannotComplete || err_num == kAXErrorAttributeUnsupported => {
            }
            _ => {
                log::info!("Simulating mouse click instead...");
                simulate_click(x, y, false);
            }
        }
    };
}

pub(crate) fn scroll_to_value(element: &AXUIElement, val: f64) {
    if let Err(e) = element.set_value(CFNumber::from(val.clamp(0.0, 1.0)).as_CFType()) {
        log::warn!("Failed to set value to the selected scroll bar: {e}.");
    };
}

pub(crate) fn right_click_menu_on_element(
    executor: &AppEngine,
    element: &AXUIElement,
    center: (f64, f64),
) {
    let (x, y) = center;

    if executor.is_electron {
        simulate_click(x, y, true);
    } else if let Err(e) = element.show_menu() {
        log::warn!("Failed to show menu on element: {e}");
        match e {
            // NOTE: Sometimes this error is false alarm, usually because it takes longer
            // than expected, we shouldn't click in this case, otherwise it is performed twice.
            accessibility::Error::Ax(err_num)
                if err_num == kAXErrorCannotComplete || err_num == kAXErrorAttributeUnsupported => {
            }
            _ => {
                log::info!("Simulating mouse click instead...");
                simulate_click(x, y, true);
            }
        }
    };
}

/// Select the parent of the currently selected element
pub(crate) fn select_parent(executor: &mut AppEngine) -> bool {
    if let Some(parent_element) = executor
        .selected
        .as_ref()
        .and_then(|eoi| eoi.element.as_ref())
        .and_then(|ele| ele.parent().ok())
    {
        let screen_frame = Frame::from_origion(executor.screen_size);
        let frame = parent_element.get_frame(screen_frame);
        executor.selected = Some(ElementOfInterest {
            element: Some(parent_element),
            context: None,
            role: RoleOfInterest::Generic,
            frame,
        });
        return true;
    }
    false
}

pub(crate) fn perform_text_action(executor: &mut AppEngine, ta: TextAction) {
    let Some(ElementOfInterest {
        context: Some(text),
        ..
    }) = executor.selected.as_ref()
    else {
        panic!("Internal Error: No selected text in Mode::TextActionMenu.");
    };

    let text = text.clone();

    // Clear old menu no matter which action is taken
    drawing::clear_drawing(executor);

    // TODO:
    // 1. URL handling
    let keep_drawing = match ta {
        TextAction::Copy => {
            crate::action::text_to_clipboard(&text);
            lifecycle::notify_then_deactivate(executor, "Copied to clipboard.", Level::Info);
            true
        }
        TextAction::Dictionary => {
            log::trace!("Looking up `{text}` in Apple Dictionary.");
            if let Some(attr_string) = get_dictionary_attributed_string(
                &text,
                &executor.config.dictionaries,
                &executor.config.theme,
            ) {
                let CGSize { width, height } = executor.screen_size;
                let (text_size, _) =
                    crate::util::estimate_frame_for_text(&attr_string, (width, height));
                executor.window.draw_attributed_string(
                    attr_string,
                    executor.screen_size,
                    text_size,
                    &executor.config.theme,
                );
            } else {
                lifecycle::notify_then_deactivate(executor, "No definition found.", Level::Warn);
            }
            true
        }
        TextAction::Split => {
            lifecycle::set_mode(executor, Mode::WordPicking);
            drawing::clear_drawing(executor);
            let word_picker = WordPicker::new(
                text,
                &executor.window,
                &executor.config.theme,
                executor.screen_size,
            );
            executor.hint_width = word_picker.digits;

            lifecycle::clear_cache(executor);
            executor.word_picker = Some(word_picker);
            true
        }
        TextAction::Editor => {
            if let Err(e) = open_editor(executor, &text) {
                lifecycle::notify_then_deactivate(
                    executor,
                    &format!("Failed to open editor: {e}"),
                    Level::Error,
                );
                true
            } else {
                false
            }
        }
        TextAction::UserDefined(idx) => {
            take_external_action(executor, idx, &text);
            true
        }
    };

    if !keep_drawing {
        lifecycle::deactivate(executor);
    }
}

pub(crate) fn perform_scroll_action(executor: &mut AppEngine, sa: ScrollAction) {
    let Some(ElementOfInterest {
        element: Some(element),
        role,
        frame,
        ..
    }) = executor.selected.as_ref()
    else {
        return;
    };

    if *role == RoleOfInterest::ScrollBar {
        let Some(old_val) = element
            .value()
            .ok()
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|f| f.to_f64())
        else {
            lifecycle::deactivate(executor);
            return;
        };

        let scroll_unit = executor.config.scroll_distance;
        match sa {
            ScrollAction::DownRight => {
                scroll_to_value(element, old_val + scroll_unit);
            }
            ScrollAction::UpLeft => {
                scroll_to_value(element, old_val - scroll_unit);
            }
            ScrollAction::IncreaseDistance => {
                executor.config.scroll_distance *= 1.5;
            }
            ScrollAction::DecreaseDistance => {
                executor.config.scroll_distance /= 1.5;
            }
            ScrollAction::Top => {
                scroll_to_value(element, 0.0);
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
            ScrollAction::Bottom => {
                scroll_to_value(element, 1.0);
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
        }
    } else {
        let distance = (frame.size().1 * executor.config.scroll_distance).max(1.0) as i64;
        match sa {
            ScrollAction::DownRight => {
                simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: -distance,
                });
            }
            ScrollAction::UpLeft => {
                simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: distance,
                });
            }
            ScrollAction::IncreaseDistance => {
                executor.config.scroll_distance *= 1.5;
            }
            ScrollAction::DecreaseDistance => {
                executor.config.scroll_distance /= 1.5;
            }
            ScrollAction::Top => {
                simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: 999999,
                });
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
            ScrollAction::Bottom => {
                simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: -999999,
                });
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
        }
    }
}

pub(crate) fn update_selected_text(executor: &mut AppEngine, new_text: String) {
    if let Some(ElementOfInterest { context, .. }) = executor.selected.as_mut() {
        *context = Some(new_text);
    }
}

pub(crate) fn update_selected_text_and_show_menu(executor: &mut AppEngine, new_text: String) {
    update_selected_text(executor, new_text);
    drawing::draw_element_menu(executor, "", &RoleOfInterest::PseudoText, true);
}

pub(crate) fn open_editor(
    executor: &mut AppEngine,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let editor = executor
        .config
        .editor
        .as_ref()
        .expect("Internal Error: No editor set.");

    // Write current selected text to temp file
    let _ = std::fs::write(&executor.temp_file, text);
    let temp_fp = executor
        .temp_file
        .to_str()
        .unwrap_or_else(|| panic!("Failed to get temp file path for {:?}.", executor.temp_file));

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

fn take_external_action(executor: &mut AppEngine, idx: usize, selected_text: &str) {
    let action = executor
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
        lifecycle::notify_then_deactivate(
            executor,
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
                update_selected_text_and_show_menu(executor, new_text);
            } else if !o.stderr.is_empty() {
                lifecycle::notify_then_deactivate(
                    executor,
                    &format!("External stderr: {}", String::from_utf8_lossy(&o.stderr)),
                    Level::Error,
                );
            }
        }
        Err(e) => {
            lifecycle::notify_then_deactivate(
                executor,
                &format!("Failed to run command: {e}"),
                Level::Error,
            );
        }
    }
}
