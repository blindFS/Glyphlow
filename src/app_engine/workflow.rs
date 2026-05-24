use super::AppEngine;
use crate::{
    ax_element::{GetAttribute, SetAttribute, Target},
    config::{RoleOfInterest, WorkFlow, WorkFlowAction},
};
use log::Level;
use rdev::{Button, EventType};
use std::time::Duration;

impl AppEngine {
    /// Check if a workflow is valid given currently selected element and app bundle id
    pub(super) fn is_workflow_valid(&self, wf: &WorkFlow) -> bool {
        if wf.valid_app_ids.as_ref().is_some_and(|ids| {
            ids.iter()
                .all(|id| *id != self.last_app_window_info.bundle_id)
        }) {
            return false;
        }
        match wf.starting_role {
            RoleOfInterest::Empty => self.selected.is_none(),
            RoleOfInterest::Generic => self
                .selected
                .as_ref()
                .is_some_and(|s| s.element().is_some()),
            _ => self
                .selected
                .as_ref()
                .is_some_and(|s| s.element().is_some() && s.role() == wf.starting_role),
        }
    }

    /// Returns true if there're pending actions to finish
    fn execute_workflow_action(&mut self, act: &WorkFlowAction) -> bool {
        // Actions don't need a selected element
        match act {
            WorkFlowAction::GlyphlowMenu => {
                self.menu_refresh("", true);
                // HACK: break the loop so the notification will be kept,
                // basically `GlyphlowMenu` should be a terminal op
                self.pending_workflow_actions.clear();
                return true;
            }
            WorkFlowAction::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                return false;
            }
            WorkFlowAction::SearchFor(ct) => {
                self.selected = None;
                self.activate(Target::Custom(ct.clone()));
                if self.element_cache.cache.len() == 1 {
                    self.clear_hints();
                    self.select(self.element_cache.cache[0].clone());
                } else if self.element_cache.cache.len() > 1 {
                    return true;
                } else {
                    // Stop on empty result
                    return true;
                }
                return false;
            }
            WorkFlowAction::KeyCombo(kb) => {
                self.set_simulating_key(true);
                for k in kb.keys.iter() {
                    self.simulate_event(&EventType::KeyPress(*k));
                    std::thread::sleep(Duration::from_millis(20));
                }
                for k in kb.keys.iter().rev() {
                    self.simulate_event(&EventType::KeyRelease(*k));
                }
                self.set_simulating_key(false);
                return false;
            }
            _ => (),
        }

        // Actions that require a selected element
        let Some(selected) = self.selected.as_ref() else {
            self.notify_then_deactivate(
                &format!("Running a workflow action with no element selected. {act:?}"),
                Level::Error,
            );
            return true;
        };

        let Some(element) = selected.element() else {
            self.notify_then_deactivate(
                &format!("Running a workflow action with no accessibility element. {act:?}"),
                Level::Error,
            );
            return true;
        };

        let context = &selected.context;
        let role = selected.role();
        let frame = selected.frame;

        match act {
            WorkFlowAction::Focus => {
                self.focus_on_element(element);
            }
            WorkFlowAction::Press => {
                let center = frame.center();
                self.press_on_element(element, &role, center);
            }
            WorkFlowAction::Hover => {
                let (x, y) = frame.center();
                self.simulate_event(&EventType::MouseMove { x, y });
            }
            WorkFlowAction::Move(x, y) => {
                self.simulate_event(&EventType::MouseMove { x: *x, y: *y });
            }
            WorkFlowAction::Click => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Left);
            }
            WorkFlowAction::RightClick => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Right);
            }
            WorkFlowAction::MiddleClick => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Middle);
            }
            WorkFlowAction::ShowMenu => {
                let center = frame.center();
                self.right_click_menu_on_element(element, center);
            }
            WorkFlowAction::GoParent => {
                if self.select_parent() {
                    self.target = Target::ChildElement;
                };
            }
            WorkFlowAction::Debug => {
                self.notify(&element.inspect(), Level::Debug);
                // HACK: break the loop so the notification will be kept,
                // basically `Debug` should be a terminal op
                self.pending_workflow_actions.clear();
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

    pub(super) fn execute_pending_workflow_actions(&mut self) {
        while let Some(act) = self.pending_workflow_actions.pop_front() {
            if self.execute_workflow_action(&act) {
                return;
            };
        }
        self.notify_then_deactivate("Done", Level::Trace);
    }

    pub(super) fn execute_workflow(&mut self, idx: usize) {
        let workflow = self
            .config
            .workflows
            .get(idx)
            .cloned()
            .expect("Internal Error: text workflow index out of bounds.");

        // Silently quit if workflow is not valid for current selected element
        if self.is_workflow_valid(&workflow) {
            self.pending_workflow_actions = workflow.actions.into();
            self.execute_pending_workflow_actions();
        }
    }
}
