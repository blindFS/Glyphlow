use super::{AppEngine, drawing, interaction, lifecycle};
use crate::{
    ax_element::{ElementOfInterest, GetAttribute, SetAttribute, Target},
    config::{RoleOfInterest, WorkFlow, WorkFlowAction},
};
use log::Level;
use rdev::EventType;
use std::time::Duration;

/// Check if a workflow's starting_role matches current selected element
pub(crate) fn is_workflow_valid(executor: &AppEngine, wf: &WorkFlow) -> bool {
    match wf.starting_role {
        RoleOfInterest::Empty => executor.selected.is_none(),
        RoleOfInterest::Generic => executor
            .selected
            .as_ref()
            .is_some_and(|s| s.element.is_some()),
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
            if interaction::select_parent(executor) {
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
