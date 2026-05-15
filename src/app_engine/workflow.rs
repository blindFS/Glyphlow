use crate::{
    Mode, ScrollAction, TextAction,
    action::{WordPicker, get_dictionary_attributed_string},
    ax_element::{ElementOfInterest, GetAttribute, SetAttribute, Target},
    config::{RoleOfInterest, WorkFlow, WorkFlowAction},
    drawer::GlyphlowDrawingLayer,
    util::Frame,
};
use log::Level;
use accessibility::AXUIElementAttributes;
use core_foundation::number::CFNumber;
use objc2_core_foundation::CGSize;
use rdev::EventType;
use std::time::Duration;
use super::{AppEngine, lifecycle, drawing, interaction};

pub(crate) fn open_editor(executor: &mut AppEngine, text: &str) -> Result<(), Box<dyn std::error::Error>> {
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

pub(crate) fn take_external_action(executor: &mut AppEngine, idx: usize, selected_text: &str) {
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
            lifecycle::notify_then_deactivate(executor, &format!("Failed to run command: {e}"), Level::Error);
        }
    }
}

/// Check if a workflow's starting_role matches current selected element
pub(crate) fn is_workflow_valid(executor: &AppEngine, wf: &WorkFlow) -> bool {
    match wf.starting_role {
        RoleOfInterest::Empty => executor.selected.is_none(),
        RoleOfInterest::Generic => executor.selected.as_ref().is_some_and(|s| s.element.is_some()),
        _ => executor
            .selected
            .as_ref()
            .is_some_and(|s| s.element.is_some() && s.role == wf.starting_role),
    }
}

/// Returns true if there're pending actions to finish
pub(crate) fn execute_workflow_action(executor: &mut AppEngine, act: &WorkFlowAction) -> bool {
    // Actions don't need a selected element
    match act {
        WorkFlowAction::GlyphlowMenu => {
            drawing::menu_refresh(executor, "", true);
            // HACK: break the loop so the notification will be kept,
            // basically `GlyphlowMenu` should be a terminal op
            executor.pending_workflow_actions.clear();
            return true;
        }
        WorkFlowAction::Sleep(ms) => {
            std::thread::sleep(Duration::from_millis(*ms));
            return false;
        }
        WorkFlowAction::SearchFor(ct) => {
            executor.selected = None;
            lifecycle::activate(executor, Target::Custom(ct.clone()));
            if executor.element_cache.cache.len() == 1 {
                drawing::clear_drawing(executor);
                executor.selected = Some(executor.element_cache.cache[0].clone());
            } else if executor.element_cache.cache.len() > 1 {
                return true;
            } else {
                // Stop on empty result
                return true;
            }
            return false;
        }
        WorkFlowAction::KeyCombo(kb) => {
            lifecycle::set_simulating_key(executor, true);
            for k in kb.keys.iter() {
                interaction::simulate_event(&EventType::KeyPress(*k));
                std::thread::sleep(Duration::from_millis(20));
            }
            for k in kb.keys.iter().rev() {
                interaction::simulate_event(&EventType::KeyRelease(*k));
            }
            lifecycle::set_simulating_key(executor, false);
            return false;
        }
        _ => (),
    }

    // Actions that require a selected element
    let Some(ElementOfInterest {
        element: Some(element),
        context,
        role,
        frame,
        ..
    }) = executor.selected.as_ref()
    else {
        lifecycle::notify_then_deactivate(
            executor,
            &format!("Running a workflow action with no element selected. {act:?}"),
            Level::Error,
        );
        return true;
    };

    match act {
        WorkFlowAction::Focus => {
            interaction::focus_on_element(element);
        }
        WorkFlowAction::Press => {
            let center = frame.center();
            interaction::press_on_element(executor, element, role, center);
        }
        WorkFlowAction::Click => {
            let (x, y) = frame.center();
            interaction::simulate_click(x, y, false);
        }
        WorkFlowAction::ShowMenu => {
            let center = frame.center();
            interaction::right_click_menu_on_element(executor, element, center);
        }
        WorkFlowAction::GoParent => {
            if select_parent(executor) {
                executor.target = Target::ChildElement;
            };
        }
        WorkFlowAction::Debug => {
            drawing::clear_drawing(executor);
            lifecycle::notify(executor, &element.inspect(), Level::Debug);
            // HACK: break the loop so the notification will be kept,
            // basically `Debug` should be a terminal op
            executor.pending_workflow_actions.clear();
            return true;
        }
        WorkFlowAction::SelectAll => {
            let len = context
                .clone()
                .map(|txt| txt.encode_utf16().count())
                .unwrap_or(0) as isize;
            element.set_selected_range(0, len);
        }
        _ => (),
    }
    false
}

pub(crate) fn execute_pending_workflow_actions(executor: &mut AppEngine) {
    drawing::clear_drawing(executor);
    while let Some(act) = executor.pending_workflow_actions.pop_front() {
        if execute_workflow_action(executor, &act) {
            return;
        };
    }
    drawing::clear_drawing(executor);
    if executor.notification_layers.is_empty() {
        lifecycle::notify_then_deactivate(executor, "Done", Level::Trace);
    }
}

pub(crate) fn execute_workflow(executor: &mut AppEngine, idx: usize) {
    let workflow = executor
        .config
        .workflows
        .get(idx)
        .cloned()
        .expect("Internal Error: text workflow index out of bounds.");

    // Silently quit if workflow is not valid for current selected element
    if is_workflow_valid(executor, &workflow) {
        executor.pending_workflow_actions = workflow.actions.into();
        execute_pending_workflow_actions(executor);
    }
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
                let (text_size, _) = crate::util::estimate_frame_for_text(&attr_string, (width, height));
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
            let word_picker =
                WordPicker::new(text, &executor.window, &executor.config.theme, executor.screen_size);
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
                interaction::scroll_to_value(element, old_val + scroll_unit);
            }
            ScrollAction::UpLeft => {
                interaction::scroll_to_value(element, old_val - scroll_unit);
            }
            ScrollAction::IncreaseDistance => {
                executor.config.scroll_distance *= 1.5;
            }
            ScrollAction::DecreaseDistance => {
                executor.config.scroll_distance /= 1.5;
            }
            ScrollAction::Top => {
                interaction::scroll_to_value(element, 0.0);
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
            ScrollAction::Bottom => {
                interaction::scroll_to_value(element, 1.0);
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
        }
    } else {
        let distance = (frame.size().1 * executor.config.scroll_distance).max(1.0) as i64;
        match sa {
            ScrollAction::DownRight => {
                interaction::simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: -distance,
                });
            }
            ScrollAction::UpLeft => {
                interaction::simulate_event(&EventType::Wheel {
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
                interaction::simulate_event(&EventType::Wheel {
                    delta_x: 0,
                    delta_y: 999999,
                });
                drawing::draw_element_menu(executor, "", &RoleOfInterest::ScrollBar, false);
            }
            ScrollAction::Bottom => {
                interaction::simulate_event(&EventType::Wheel {
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

pub(crate) fn toggle_multiselection(executor: &mut AppEngine) {
    executor.multi_selection.toggle();
    let on_off = if executor.multi_selection.is_on {
        "on"
    } else {
        "off"
    };
    lifecycle::notify(executor, &format!("Multi-selection is now {on_off}."), Level::Info);
}
