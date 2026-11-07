//! Current state follows original quiescent or active Root/worker custody.
//! A copied envelope grants no resume, endpoint or graph publication authority.
use super::*;
use crate::plugin_worker::{
    NativeParameterHold, NativeWorkerCapture, NativeWorkerCaptureProgress,
    NativeWorkerControllerProgress, NativeWorkerControllerQuery, NativeWorkerParameterChange,
};
#[cfg(test)]
mod tests;

impl Custody {
    // Work excludes source replacement/return; bank is held only for admission
    // and terminal context checks, never copied-state allocation or worker I/O.
    fn with_live_capture<R>(
        &self,
        binding: Binding,
        run: impl FnOnce(&mut Owner) -> Result<R, &'static str>,
    ) -> Result<R, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let index = {
            let bank = lock(&self.bank);
            if !self.enabled() || bank.closing {
                return Err("native capture is closing");
            }
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &bank.sources.slots[index];
            let cell = &work.sources.cells[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.cancellation_requested
                || slot.cancellation.is_none()
                || slot.facts.is_none()
                || slot.presentation.is_none()
                || cell.pending
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native capture source is not active");
            }
            let matches = match cell.owner.as_ref() {
                Some(Owner::Effect(owner)) => {
                    owner.binding() == binding
                        && slot.identity == Some(owner.identity())
                        && slot.processor == Some(owner.prepared())
                        && !slot.endpoint_loaned
                        && slot.endpoint_return.is_none()
                        && owner.constructor_storage_on_control().is_some()
                }
                Some(Owner::Loaned(owner)) => {
                    owner.binding() == binding
                        && slot.identity == Some(owner.identity())
                        && slot.processor == Some(owner.prepared())
                        && slot.endpoint_loaned
                        && slot.endpoint_return.is_some()
                        && owner.constructor_storage_on_control().is_some()
                }
                _ => false,
            };
            if !matches {
                return Err("native capture requires original active effect custody");
            }
            index
        };
        let owner = work.sources.cells[index]
            .owner
            .as_mut()
            .ok_or("native capture owner missing")?;
        let result = run(owner)?;
        let current = {
            let bank = lock(&self.bank);
            let slot = &bank.sources.slots[index];
            !bank.closing
                && slot.binding == Some(binding)
                && slot.phase == Phase::Accepted
                && !slot.retiring
                && !slot.starting
                && !slot.cancellation_requested
        };
        if !current {
            drop(result); // Copied payload/token destruction outside bank.
            return Err("native capture source is closing");
        }
        Ok(result)
    }
    // Once an input is admitted, return its token even if the bank closes
    // during the final live-source check. The caller owns the exact terminal.
    fn admit_live_operation<R>(
        &self,
        binding: Binding,
        run: impl FnOnce(&mut Owner) -> Result<R, &'static str>,
        after_admission: impl FnOnce(),
        refused: &'static str,
    ) -> Result<R, &'static str> {
        let mut admitted = None;
        let result = self.with_live_capture(binding, |owner| {
            admitted = Some(run(owner)?);
            after_admission();
            Ok(())
        });
        match admitted {
            Some(token) => Ok(token),
            None => result.and(Err(refused)),
        }
    }

    pub(in crate::candidate_custody) fn native_parameter_hold(
        &self,
        binding: Binding,
        command: NativeParameterHold<'_>,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.parameter_hold_admission(binding, command, || {})
    }

    fn parameter_hold_admission(
        &self,
        binding: Binding,
        command: NativeParameterHold<'_>,
        after_admission: impl FnOnce(),
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.admit_live_operation(
            binding,
            |owner| match owner {
                Owner::Effect(owner) => owner.parameter_hold(command),
                Owner::Loaned(owner) => owner.parameter_hold(command),
                _ => Err("native parameter input owner differs"),
            },
            after_admission,
            "native parameter hold was not admitted",
        )
    }

    pub(in crate::candidate_custody) fn native_parameter_edits(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::ParameterEdits>, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.poll_parameter_edits(),
            Owner::Loaned(owner) => owner.poll_parameter_edits(),
            _ => Err("native edit owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn native_recovery_edits(
        &self,
        binding: Binding,
        flush_idle: bool,
    ) -> Result<Option<hex_plugin_host::protocol::RecoveryEdits>, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.poll_recovery_edits(flush_idle),
            Owner::Loaned(owner) => owner.poll_recovery_edits(flush_idle),
            _ => Err("native edit owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn native_parameter_values(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_worker::NativeParameterValues, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.parameter_values(),
            Owner::Loaned(owner) => owner.parameter_values(),
            _ => Err("native parameter owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn begin_native_parameter_change(
        &self,
        binding: Binding,
        id: u32,
        normalized: f64,
        idle: bool,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        self.parameter_change_admission(binding, id, normalized, idle, || {})
    }
    fn parameter_change_admission(
        &self,
        binding: Binding,
        id: u32,
        normalized: f64,
        idle: bool,
        after_admission: impl FnOnce(),
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.admit_live_operation(
            binding,
            |owner| match owner {
                Owner::Effect(owner) => owner.begin_parameter_change(id, normalized, idle),
                Owner::Loaned(owner) => owner.begin_parameter_change(id, normalized, idle),
                _ => Err("native parameter input owner differs"),
            },
            after_admission,
            "native parameter point was not admitted",
        )
    }
    pub(in crate::candidate_custody) fn withdraw_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<bool, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.withdraw_parameter_change(request),
            Owner::Loaned(owner) => owner.withdraw_parameter_change(request),
            _ => Err("native parameter input owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn poll_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.poll_parameter_change(request),
            Owner::Loaned(owner) => owner.poll_parameter_change(request),
            _ => Err("native parameter input owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn drain_stopped_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.drain_stopped_parameter_change(request),
            Owner::Loaned(owner) => owner.drain_stopped_parameter_change(request),
            _ => Err("native parameter input owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn begin_native_controller_query(
        &self,
        binding: Binding,
        query: impl Into<hex_plugin_host::protocol::ControllerRequest>,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.controller_query_admission(binding, query, || {})
    }
    fn controller_query_admission(
        &self,
        binding: Binding,
        query: impl Into<hex_plugin_host::protocol::ControllerRequest>,
        after_admission: impl FnOnce(),
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        let query = query.into();
        self.admit_live_operation(
            binding,
            |owner| match owner {
                Owner::Effect(owner) => owner.begin_controller_query(query),
                Owner::Loaned(owner) => owner.begin_controller_query(query),
                _ => Err("native controller owner differs"),
            },
            after_admission,
            "native controller query was not admitted",
        )
    }
    pub(in crate::candidate_custody) fn poll_native_controller_query(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.poll_controller_query(request),
            Owner::Loaned(owner) => owner.poll_controller_query(request),
            _ => Err("native controller owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn begin_live_native_capture(
        &self,
        binding: Binding,
    ) -> Result<NativeWorkerCapture, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.begin_state_capture(),
            Owner::Loaned(owner) => owner.begin_state_capture(),
            _ => Err("native capture owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn poll_live_native_capture(
        &self,
        binding: Binding,
        request: &NativeWorkerCapture,
    ) -> Result<NativeWorkerCaptureProgress, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.poll_state_capture(request),
            Owner::Loaned(owner) => owner.poll_state_capture(request),
            _ => Err("native capture owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn capture_native_source(
        &self,
        binding: Binding,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<NativeDeviceState>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            owner.capture_state_retained_while_on_control(is_cancelled)
        })
    }
    pub(in crate::candidate_custody) fn flush_native_parameters(
        &self,
        binding: Binding,
        parameters: &hex_plugin_host::transport::Parameters,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<hex_plugin_host::protocol::FlushedParameters>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            owner.flush_parameters_retained_while_on_control(parameters, is_cancelled)
        })
    }
    pub(in crate::candidate_custody) fn reactivate_native_source(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            let owner = owner.reactivate_retained_on_control()?;
            let facts = owner.facts();
            Ok((owner, facts))
        })
    }
    pub(in crate::candidate_custody) fn settle_native_latency(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            let owner = owner.settle_latency_retained_on_control()?;
            let facts = owner.facts();
            Ok((owner, facts))
        })
    }
    // Capture, parameter flush and reactivation use the same quiescent-owner
    // admission/terminal path. Only Work spans SDK I/O; close keeps its existing
    // exact cancellation token, and neither operation grants resume/publication.
    fn with_quiescent_native_source<T>(
        &self,
        binding: Binding,
        run: impl FnOnce(
            native::Quiescent,
        ) -> Result<(native::Quiescent, T), native::PreparationFailure>,
    ) -> Result<Option<T>, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let index = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing {
                return Err("native capture is closing");
            }
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let slot = &bank.sources.slots[index];
            let cell = &work.sources.cells[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.endpoint_loaned
                || slot.endpoint_return.is_some()
                || slot.cancellation_requested
                || slot.cancellation.is_none()
                || slot.processor.is_some()
                || slot.presentation.is_some()
                || bank.publication.references_native_source(index)
                || cell.pending
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native capture requires original quiescent source custody");
            }
            let Some(Owner::Quiescent(owner)) = cell.owner.as_ref() else {
                return Err("native capture owner is not quiescent");
            };
            let previous = owner.configuration().binding;
            if previous.root != binding.root
                || previous.instance != binding.instance
                || previous.graph > binding.graph
                || previous.epoch >= binding.epoch
                || slot
                    .identity
                    .is_none_or(|identity| identity.device_id() != binding.instance)
                || slot.facts != Some(owner.facts())
                || owner.retained_storage_on_control().is_none()
            {
                return Err("native capture source reservation differs");
            }
            bank.sources.slots[index].starting = true;
            index
        };
        let cell = &mut work.sources.cells[index];
        cell.pending = true;
        // Only Work spans native control I/O. The ordinary bank is free so
        // close may cancel this exact source through its original token.
        let mut captured = None;
        let result = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Quiescent(owner)) => match run(owner) {
                Ok((owner, state)) => {
                    cell.owner = Some(Owner::Quiescent(owner));
                    captured = Some(state);
                }
                Err(owner) => {
                    cell.constructor_refused = true;
                    cell.owner = Some(Owner::Refused(owner));
                }
            },
            other => cell.owner = other,
        }));
        if let Err(payload) = result {
            cell.first_panic = Some(payload);
            self.fail_native_source(index);
            return Ok(None);
        }
        cell.pending = false;
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.retiring |= closing || captured.is_none();
        if let Some(Owner::Quiescent(owner)) = cell.owner.as_ref() {
            // Reactivation can change facts. Install them with the retained
            // owner before completion is allowed to resume this reservation.
            slot.facts = Some(owner.facts());
        } else {
            slot.phase = Phase::Cleaning;
        }
        drop(bank);
        if closing {
            captured = None;
        }
        Ok(captured)
    }
}

impl CandidateRoot {
    pub(crate) fn drain_stopped_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .drain_stopped_native_parameter_change(binding, request)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn reactivate_native_source(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.reactivate_native_source(binding);
        crate::lifecycle::request_candidate_cleanup(self);
        self.require_native_source_control_open()?;
        result
    }

    pub(crate) fn settle_native_latency(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.settle_native_latency(binding);
        crate::lifecycle::request_candidate_cleanup(self);
        self.require_native_source_control_open()?;
        result
    }
    pub(crate) fn native_source_parameter_edits(
        &self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::ParameterEdits>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.native_parameter_edits(binding)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn native_source_recovery_edits(
        &self,
        binding: Binding,
        flush_idle: bool,
    ) -> Result<Option<hex_plugin_host::protocol::RecoveryEdits>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.native_recovery_edits(binding, flush_idle)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn native_source_parameter_values(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_worker::NativeParameterValues, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.native_parameter_values(binding)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn native_parameter_hold(
        &self,
        binding: Binding,
        command: NativeParameterHold<'_>,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.require_native_source_control_open()?;
        self.0.eligible.native_parameter_hold(binding, command)
    }

    pub(crate) fn begin_native_parameter_change(
        &self,
        binding: Binding,
        id: u32,
        normalized: f64,
        idle: bool,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        self.require_native_source_control_open()?;
        self.0
            .eligible
            .begin_native_parameter_change(binding, id, normalized, idle)
    }
    pub(crate) fn withdraw_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<bool, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .withdraw_native_parameter_change(binding, request)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn poll_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .poll_native_parameter_change(binding, request)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn begin_native_controller_query(
        &self,
        binding: Binding,
        query: impl Into<hex_plugin_host::protocol::ControllerRequest>,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.require_native_source_control_open()?;
        self.0
            .eligible
            .begin_native_controller_query(binding, query)
    }
    pub(crate) fn poll_native_controller_query(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .poll_native_controller_query(binding, request)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn begin_live_native_capture(
        &self,
        binding: Binding,
    ) -> Result<NativeWorkerCapture, &'static str> {
        self.require_native_source_control_open()?;
        self.0.eligible.begin_live_native_capture(binding)
    }
    pub(crate) fn poll_live_native_capture(
        &self,
        binding: Binding,
        request: &NativeWorkerCapture,
    ) -> Result<NativeWorkerCaptureProgress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.poll_live_native_capture(binding, request)?;
        // Constructor close precedes eligible-bank close. Recheck this outer
        // gate too; copied result destruction occurs after its lock is gone.
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn flush_native_source_parameters(
        &self,
        binding: Binding,
        parameters: &hex_plugin_host::transport::Parameters,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<hex_plugin_host::protocol::FlushedParameters>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .flush_native_parameters(binding, parameters, is_cancelled);
        crate::lifecycle::request_candidate_cleanup(self);
        self.require_native_source_control_open()?;
        result
    }
    pub(crate) fn capture_native_source_state(
        &self,
        binding: Binding,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<NativeDeviceState>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.capture_native_source(binding, is_cancelled);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
}
