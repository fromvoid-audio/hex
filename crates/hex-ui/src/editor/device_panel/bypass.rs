//! Display custody for ordered device bypass toggles.
//!
//! The project owns the actual state. Each accepted toggle keeps its visual
//! effect until the corresponding worker terminal installs canonical state.
use super::super::{
    EditorView,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{Command, DeviceId};
use hex_session::service::{Event, Request};

#[derive(Default)]
pub(super) struct State {
    pending: Vec<Pending>,
}

struct Pending {
    device: DeviceId,
    receipt: Receipt,
}

impl State {
    pub(in crate::editor) fn displayed(&self, device: DeviceId, canonical: bool) -> bool {
        canonical ^ (self.pending.iter().filter(|p| p.device == device).count() % 2 == 1)
    }
}

impl EditorView {
    pub(in crate::editor) fn only_bypass_requests_pending(&self) -> bool {
        let pending = &self.device_panel.bypass.pending;
        self.worker_requests_pending == pending.iter().filter(|p| !p.receipt.is_queued()).count()
            && self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .all(|r| matches!(r, Request::Edit(Command::ToggleDeviceBypass { .. })))
    }

    pub(in crate::editor) fn retire_abandoned_bypass(&mut self) -> bool {
        if !self
            .device_panel
            .bypass
            .pending
            .iter()
            .any(|p| p.receipt.is_queued())
        {
            return false;
        }
        let mut queued = self
            .pending_requests
            .iter()
            .chain(self.device_gesture_barriers.iter())
            .filter_map(|r| match r {
                Request::Edit(Command::ToggleDeviceBypass { device }) => Some(*device),
                _ => None,
            })
            .collect::<Vec<_>>();
        let before = self.device_panel.bypass.pending.len();
        self.device_panel.bypass.pending.retain(|p| {
            if !p.receipt.is_queued() {
                return true;
            }
            if let Some(index) = queued.iter().position(|device| *device == p.device) {
                queued.remove(index);
                true
            } else {
                false
            }
        });
        before != self.device_panel.bypass.pending.len()
    }

    pub(in crate::editor) fn toggle_device_bypass(&mut self, device: DeviceId) {
        self.device_panel.bypass.pending.push(Pending {
            device,
            receipt: Receipt::new(self.project_generation),
        });
        if !self.enqueue_device_barrier(Request::Edit(Command::ToggleDeviceBypass { device })) {
            // Admission may already have retired the matching queued receipt.
            if let Some(index) = self
                .device_panel
                .bypass
                .pending
                .iter()
                .rposition(|p| p.device == device && p.receipt.is_queued())
            {
                self.device_panel.bypass.pending.remove(index);
            }
        }
    }

    pub(in crate::editor) fn pending_bypass_matches(&self, request: &Request) -> bool {
        let Request::Edit(Command::ToggleDeviceBypass { device }) = request else {
            return false;
        };
        self.device_panel
            .bypass
            .pending
            .iter()
            .any(|p| p.device == *device && p.receipt.is_queued())
    }

    pub(in crate::editor) fn bypass_submitted(&mut self, request: &Request) {
        let Request::Edit(Command::ToggleDeviceBypass { device }) = request else {
            return;
        };
        if let Some(pending) = self
            .device_panel
            .bypass
            .pending
            .iter_mut()
            .find(|p| p.device == *device && p.receipt.is_queued())
        {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(in crate::editor) fn retire_unsubmitted_bypass(&mut self, request: &Request) -> bool {
        let Request::Edit(Command::ToggleDeviceBypass { device }) = request else {
            return false;
        };
        if let Some(index) = self
            .device_panel
            .bypass
            .pending
            .iter()
            .position(|p| p.device == *device && p.receipt.is_queued())
        {
            self.device_panel.bypass.pending.remove(index);
            true
        } else {
            false
        }
    }

    pub(in crate::editor) fn clear_pending_bypass(&mut self) -> bool {
        let changed = !self.device_panel.bypass.pending.is_empty();
        self.device_panel.bypass.pending.clear();
        changed
    }

    pub(in crate::editor) fn bypass_request_complete(&mut self, event: &Event) {
        self.device_panel.bypass.pending.retain_mut(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) == Disposition::Waiting
        });
    }
}
