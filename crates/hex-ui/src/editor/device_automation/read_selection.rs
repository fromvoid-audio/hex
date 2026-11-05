//! Read selection is queried from canonical/accepted preview state. The only
//! local selection overlay is an exact, source-stamped pending edit receipt.
use super::*;
use hex_lattice_contract::ParameterId;
use hex_project::AutomationReadSelection;
use hex_session::service::DeviceAutomationControlEdit;

#[derive(Default)]
pub(super) struct ReadSummary(std::cell::RefCell<Option<(std::sync::Weak<Project>, bool)>>);
impl ReadSummary {
    fn has_overrides(&self, project: &std::sync::Arc<Project>) -> bool {
        if let Some((source, value)) = &*self.0.borrow()
            && source.as_ptr() == std::sync::Arc::as_ptr(project)
        {
            return *value;
        }
        let value = project.has_overridden_device_automation();
        *self.0.borrow_mut() = Some((std::sync::Arc::downgrade(project), value));
        value
    }
}

pub(super) struct PendingRead {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) edit: DeviceAutomationControlEdit,
    pub(super) receipt: Receipt,
}

impl EditorView {
    fn automation_read_project(&self) -> Option<&std::sync::Arc<Project>> {
        let state = self.state.as_ref()?;
        if let Some(active) = self.device_gesture.snapshot()
            && let Some(preview) = self.resident_device_preview.as_ref().filter(|preview| {
                preview.identity.key == active.key
                    && preview.identity.sequence <= active.sequence
                    && preview.identity.project_generation == self.project_generation
                    && preview.identity.canonical_revision == state.revision
            })
        {
            return Some(&preview.state.project);
        }
        Some(&state.project)
    }
    pub(in crate::editor) fn device_automation_read_selection(
        &self,
        target: Target,
    ) -> Option<AutomationReadSelection> {
        let project = self.automation_read_project()?;
        target.current(project, self.project_generation)?;
        let selection = project
            .device(target.device)?
            .automation_target(target.kind)?
            .read_selection;
        if let Some(pending) = &self.automation_view.devices.read_pending
            && pending.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == pending.revision)
        {
            match pending.edit {
                DeviceAutomationControlEdit::ReadSelection {
                    device,
                    target: kind,
                    selection,
                } if device == target.device && kind == target.kind => return Some(selection),
                DeviceAutomationControlEdit::Reenable => {
                    return Some(AutomationReadSelection::Authored);
                }
                _ => {}
            }
        }
        Some(selection)
    }
    pub(in crate::editor) fn has_overridden_device_automation(&self) -> bool {
        self.automation_read_project().is_some_and(|project| {
            self.automation_view
                .devices
                .read_summary
                .has_overrides(project)
        })
    }
    pub(in crate::editor) fn manual_device_parameter_request(
        &self,
        device: DeviceId,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<Request, &'static str> {
        let state = self.state.as_ref().ok_or("project is unavailable")?;
        Ok(Request::DeviceAutomationControl {
            project_generation: self.project_generation,
            revision: state.revision,
            edit: DeviceAutomationControlEdit::ManualParameter {
                device,
                parameter,
                value,
            },
        })
    }
    pub(in crate::editor) fn reenable_device_automation(
        &mut self,
        target: Option<Target>,
        cx: &mut Context<Self>,
    ) {
        if self.edit_pending()
            || self.automation_view.devices.pending.is_some()
            || self.automation_view.devices.read_pending.is_some()
            || !super::super::device_panel::finalize_controls(self, cx)
        {
            return;
        }
        let edit = if let Some(target) = target {
            if self.device_automation_read_selection(target) != Some(AutomationReadSelection::Base)
            {
                return;
            }
            DeviceAutomationControlEdit::ReadSelection {
                device: target.device,
                target: target.kind,
                selection: AutomationReadSelection::Authored,
            }
        } else {
            if !self.has_overridden_device_automation() {
                return;
            }
            DeviceAutomationControlEdit::Reenable
        };
        let Some(state) = &self.state else {
            return;
        };
        let generation = self.project_generation;
        let revision = state.revision;
        let request = Request::DeviceAutomationControl {
            project_generation: generation,
            revision,
            edit: edit.clone(),
        };
        self.automation_view.devices.read_pending = Some(PendingRead {
            generation,
            revision,
            edit,
            receipt: Receipt::new(generation),
        });
        if !self.enqueue(request) {
            self.automation_view.devices.read_pending = None;
        }
        cx.notify();
    }
}
