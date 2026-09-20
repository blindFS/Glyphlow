use super::AppEngine;
use crate::{
    Mode,
    ax_element::{CompiledTarget, GetAttribute, SetAttribute, Target},
    config::{CustomTarget, RoleOfInterest, WorkFlow, WorkFlowAction},
};
use log::Level;
use monio::Button;
use std::time::Duration;

impl AppEngine {
    /// Check if a workflow is valid given currently selected element and app bundle id
    pub(super) fn is_workflow_valid(&self, wf: &WorkFlow) -> bool {
        self.workflow_invalid_reason(wf).is_none()
    }

    /// Whether the workflow's `valid_app_ids` restriction (if any) admits the
    /// currently focused app.
    fn app_id_allows(&self, wf: &WorkFlow) -> bool {
        !wf.valid_app_ids.as_ref().is_some_and(|ids| {
            ids.iter()
                .all(|id| *id != self.last_app_window_info.bundle_id)
        })
    }

    /// Whether the current selection satisfies the workflow's `starting_role`.
    fn role_satisfied(&self, wf: &WorkFlow) -> bool {
        match wf.starting_role {
            RoleOfInterest::Any => true,
            RoleOfInterest::Some => self.selected.is_some(),
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

    /// Why `wf` cannot run against the current selection, if it cannot.
    fn workflow_invalid_reason(&self, wf: &WorkFlow) -> Option<String> {
        let name = wf.display.trim();
        if !self.app_id_allows(wf) {
            return Some(format!(
                "Workflow `{name}` does not apply to `{}`",
                self.last_app_window_info.bundle_id
            ));
        }
        if self.role_satisfied(wf) {
            return None;
        }
        let current = self
            .selected
            .as_ref()
            .map(|s| format!("{:?}", s.role()))
            .unwrap_or_else(|| "nothing".to_string());
        Some(format!(
            "Workflow `{name}` needs a {:?} selection, current selection is {current}",
            wf.starting_role
        ))
    }

    /// Returns true if there're pending actions to finish
    fn execute_workflow_action(&mut self, act: &WorkFlowAction) -> bool {
        // Actions don't need a selected element
        match act {
            WorkFlowAction::GlyphlowMenu => {
                if self.selected.is_some() {
                    self.menu_refresh("", true);
                } else {
                    self.draw_dashboard("");
                    self.set_mode(Mode::DashBoard);
                }
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
                match CompiledTarget::new(ct) {
                    Ok(compiled) => self.activate(Target::Custom(Box::new(compiled))),
                    Err(e) => {
                        log::error!("Invalid regex in SearchFor target: {e}");
                        return true;
                    }
                }
                if self.element_cache.cache.len() == 1 {
                    self.clear_hints();
                    self.select(self.element_cache.cache[0].clone());
                } else {
                    // Stop on empty result/multiple results
                    return true;
                }
                return false;
            }
            WorkFlowAction::Move(x, y) => {
                self.move_mouse_with_trail(*x, *y);
                return false;
            }
            WorkFlowAction::KeyCombo(kb) => {
                self.set_simulating_key(true);
                for k in kb.keys.iter() {
                    let _ = monio::key_press(*k);
                    std::thread::sleep(Duration::from_millis(20));
                }
                for k in kb.keys.iter().rev() {
                    let _ = monio::key_release(*k);
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

        let frame = selected.frame;
        match act {
            WorkFlowAction::Hover => {
                let (x, y) = frame.center();
                self.move_mouse_with_trail(x, y);
                return false;
            }
            WorkFlowAction::Click => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Left);
                return false;
            }
            WorkFlowAction::RightClick => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Right);
                return false;
            }
            WorkFlowAction::MiddleClick => {
                let (x, y) = frame.center();
                self.simulate_click(x, y, Button::Middle);
                return false;
            }
            _ => (),
        }

        // Actions that require an AX element
        let Some(element) = selected.element() else {
            self.notify_then_deactivate(
                &format!("Running a workflow action with no accessibility element. {act:?}"),
                Level::Error,
            );
            return true;
        };

        let context = &selected.context;
        let role = selected.role();

        match act {
            WorkFlowAction::Focus => {
                self.focus_on_element(element);
            }
            WorkFlowAction::Press => {
                let center = frame.center();
                self.press_on_element(element, &role, center);
            }
            WorkFlowAction::ShowMenu => {
                let center = frame.center();
                self.right_click_menu_on_element(element, center);
            }
            WorkFlowAction::GoParent => {
                let flag = self.select_parent();
                if flag {
                    self.target = Target::ChildElement;
                }
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
                    .as_ref()
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
            .expect("Internal Error: workflow index out of bounds.");

        if self.is_workflow_valid(workflow) {
            self.pending_workflow_actions = workflow.actions.clone().into();
            self.execute_pending_workflow_actions();
        } else {
            self.draw_menu("Wrong key sequence\nPress 󰁮 to go back");
        }
    }

    /// Run a workflow addressed by (a fragment of) its display name.
    ///
    /// When the workflow is only blocked by its `starting_role`, the elements
    /// matching that role are offered for picking and the workflow runs on
    /// whichever one gets picked. This reuses the very same machinery as the
    /// `SearchFor` action: a custom target plus queued workflow actions.
    pub(super) fn run_workflow_by_name(&mut self, name: &str) {
        let Some(idx) = self.config.find_workflow(name) else {
            let msg = format!(
                "No workflow matches `{name}`. You can check available ones with `glyphlow-cli workflow list`."
            );
            self.notify_then_deactivate(&msg, Level::Error);
            return;
        };

        if self.is_workflow_valid(&self.config.workflows[idx]) {
            self.pending_workflow_actions = self.config.workflows[idx].actions.clone().into();
            self.execute_pending_workflow_actions();
            return;
        }

        // An app restriction is a hard no; only a role mismatch is recoverable
        // by letting the user pick an element.
        let recoverable = self.app_id_allows(&self.config.workflows[idx]);
        let role_target = custom_target_for_role(self.config.workflows[idx].starting_role);

        if recoverable
            && let Some(ct) = role_target
            && let Ok(compiled) = CompiledTarget::new(&ct)
        {
            self.pending_workflow_actions = self.config.workflows[idx].actions.clone().into();
            self.activate(Target::Custom(Box::new(compiled)));
            // Unambiguous pick: skip the hints and run straight away.
            if self.element_cache.cache.len() == 1 {
                self.clear_hints();
                self.select(self.element_cache.cache[0].clone());
                self.execute_pending_workflow_actions();
            }
            return;
        }

        if let Some(reason) = self.workflow_invalid_reason(&self.config.workflows[idx]) {
            self.notify_then_deactivate(&reason, Level::Error);
        }
    }
}

/// A [`CustomTarget`] matching the accessibility roles behind a workflow's
/// `starting_role`, used to offer the right elements for picking.
fn custom_target_for_role(role: RoleOfInterest) -> Option<CustomTarget> {
    let role = match role {
        RoleOfInterest::Button => "button",
        RoleOfInterest::CheckBox => "checkbox",
        RoleOfInterest::Image => "image",
        RoleOfInterest::MenuItem => "menuitem",
        RoleOfInterest::ScrollBar => "scrollbar",
        RoleOfInterest::StaticText => "statictext|heading",
        RoleOfInterest::TextField => "textfield|textarea|combobox",
        RoleOfInterest::Cell => "cell",
        _ => return None,
    };
    Some(CustomTarget {
        role: role.to_string(),
        ..Default::default()
    })
}
