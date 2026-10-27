//! Current state follows original quiescent or active Root/worker custody.
//! A copied envelope grants no resume, endpoint or graph publication authority.
use super::*;
use crate::plugin_worker::{
    NativeWorkerCapture, NativeWorkerCaptureProgress, NativeWorkerControllerProgress,
    NativeWorkerControllerQuery,
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
    pub(in crate::candidate_custody) fn matches_native_accepted_state(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => {
                NativeEffectReference::Unsplit(owner).matches_accepted_state(state)
            }
            Owner::Loaned(owner) => {
                NativeEffectReference::Loaned(owner).matches_accepted_state(state)
            }
            _ => Err("native accepted state owner differs"),
        })
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
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        self.parameter_admission(binding, id, normalized, || {})
    }

    fn parameter_admission(
        &self,
        binding: Binding,
        id: u32,
        normalized: f64,
        after_admission: impl FnOnce(),
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        let mut admitted = None;
        let result = self.with_live_capture(binding, |owner| {
            admitted = Some(match owner {
                Owner::Effect(owner) => owner.begin_parameter_change(id, normalized)?,
                Owner::Loaned(owner) => owner.begin_parameter_change(id, normalized)?,
                _ => return Err("native parameter input owner differs"),
            });
            after_admission();
            Ok(())
        });
        // Once the worker admits an input, a racing close cannot turn that
        // admission into a refusal and discard its only correlation token.
        match admitted {
            Some(request) => Ok(request),
            None => result.and(Err("native parameter input was not admitted")),
        }
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
    ) -> Result<crate::plugin_worker::NativeParameterPoll, &'static str> {
        self.parameter_terminal_receipt(
            binding,
            false,
            || {},
            |owner| match owner {
                Owner::Effect(owner) => owner.poll_parameter_change(request),
                Owner::Loaned(owner) => owner.poll_parameter_change(request),
                _ => Err("native parameter input owner differs"),
            },
        )
    }
    pub(in crate::candidate_custody) fn drain_stopped_native_parameter_change(
        &self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeParameterPoll, &'static str> {
        self.parameter_terminal_receipt(
            binding,
            true,
            || {},
            |owner| match owner {
                Owner::Effect(owner) => owner.drain_stopped_parameter_change(request),
                Owner::Loaned(owner) => owner.drain_stopped_parameter_change(request),
                _ => Err("native parameter input owner differs"),
            },
        )
    }

    // The worker can consume a terminal before the post-poll bank check sees
    // close. Preserve that exact terminal as data, while marking its source
    // invalid for any later canonical or publication admission.
    fn parameter_terminal_receipt(
        &self,
        binding: Binding,
        stopped: bool,
        after_terminal: impl FnOnce(),
        run: impl FnOnce(
            &mut Owner,
        )
            -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str>,
    ) -> Result<crate::plugin_worker::NativeParameterPoll, &'static str> {
        use crate::plugin_worker::{
            NativeParameterPoll as Poll, NativeWorkerParameterProgress as Progress,
        };
        let mut consumed = None;
        let result = self.with_live_capture(binding, |owner| {
            let progress = run(owner)?;
            let poll = match progress {
                Progress::Pending => Poll::Pending,
                Progress::Failed if !stopped => Poll::TransportRevoked,
                terminal => {
                    consumed = Some(terminal);
                    after_terminal();
                    Poll::terminal(terminal)
                }
            };
            Ok(poll)
        });
        match (result, consumed) {
            (Err(reason), Some(terminal)) => Poll::terminal(terminal).source_closed(reason),
            (result, _) => result,
        }
    }
    pub(in crate::candidate_custody) fn begin_native_controller_query(
        &self,
        binding: Binding,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.controller_query_admission(binding, query, || {})
    }

    fn controller_query_admission(
        &self,
        binding: Binding,
        query: hex_plugin_host::protocol::ParameterPageQuery,
        after_admission: impl FnOnce(),
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        let mut admitted = None;
        let result = self.with_live_capture(binding, |owner| {
            admitted = Some(match owner {
                Owner::Effect(owner) => owner.begin_controller_query(query)?,
                Owner::Loaned(owner) => owner.begin_controller_query(query)?,
                _ => return Err("native controller owner differs"),
            });
            after_admission();
            Ok(())
        });
        match admitted {
            Some(request) => Ok(request),
            None => result.and(Err("native controller query was not admitted")),
        }
    }
    pub(in crate::candidate_custody) fn poll_native_controller_query(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        self.controller_query_receipt(binding, request, || {})
    }

    fn controller_query_receipt(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
        after_terminal: impl FnOnce(),
    ) -> Result<NativeWorkerControllerProgress, &'static str> {
        let mut consumed = false;
        let result = self.with_live_capture(binding, |owner| {
            let progress = match owner {
                Owner::Effect(owner) => owner.poll_controller_query(request)?,
                Owner::Loaned(owner) => owner.poll_controller_query(request)?,
                _ => return Err("native controller owner differs"),
            };
            if !matches!(
                progress,
                NativeWorkerControllerProgress::Pending | NativeWorkerControllerProgress::Failed
            ) {
                consumed = true;
                after_terminal();
            }
            Ok(progress)
        });
        match result {
            Err(_) if consumed => Ok(NativeWorkerControllerProgress::Discarded),
            result => result,
        }
    }
    pub(in crate::candidate_custody) fn drain_stopped_native_controller_query(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
    ) -> Result<bool, &'static str> {
        let mut drained = false;
        let result = self.with_live_capture(binding, |owner| {
            drained = match owner {
                Owner::Effect(owner) => owner.drain_stopped_controller_query(request)?,
                Owner::Loaned(owner) => owner.drain_stopped_controller_query(request)?,
                _ => return Err("native controller owner differs"),
            };
            Ok(())
        });
        // Once consumed, a racing close cannot make the same token pending.
        if drained {
            Ok(true)
        } else {
            result.map(|()| false)
        }
    }
    pub(in crate::candidate_custody) fn begin_live_native_capture(
        &self,
        binding: Binding,
    ) -> Result<NativeWorkerCapture, &'static str> {
        self.capture_admission(binding, || {})
    }
    fn capture_admission(
        &self,
        binding: Binding,
        after_admission: impl FnOnce(),
    ) -> Result<NativeWorkerCapture, &'static str> {
        let mut admitted = None;
        let result = self.with_live_capture(binding, |owner| {
            admitted = Some(match owner {
                Owner::Effect(owner) => owner.begin_state_capture()?,
                Owner::Loaned(owner) => owner.begin_state_capture()?,
                _ => return Err("native capture owner differs"),
            });
            after_admission();
            Ok(())
        });
        match admitted {
            Some(request) => Ok(request),
            None => result.and(Err("native capture was not admitted")),
        }
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
    pub(in crate::candidate_custody) fn stage_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        self.stage_snapshot_receipt(binding, snapshot, || {})
    }
    fn stage_snapshot_receipt(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        after_stage: impl FnOnce(),
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        let mut staged = false;
        let result = self.with_live_capture(binding, |owner| {
            let max_data = lock(&self.bank)
                .sources
                .limits
                .ok_or("native source data is not reserved")?
                .max_retained_data_bytes;
            match owner {
                Owner::Effect(owner) => owner.stage_state_snapshot(snapshot, max_data),
                Owner::Loaned(owner) => {
                    let max_data = max_data
                        .checked_sub(
                            NativeEndpointEnvelope::loan_data_bytes()
                                .ok_or("native endpoint data overflow")?,
                        )
                        .ok_or("native state approval data reservation exceeded")?;
                    owner.stage_state_snapshot(snapshot, max_data)
                }
                _ => Err("native snapshot owner differs"),
            }?;
            staged = true;
            after_stage();
            Ok(crate::plugin_worker::NativeSnapshotAction::Current)
        });
        match result {
            Err(_) if staged => Ok(crate::plugin_worker::NativeSnapshotAction::SourceClosed),
            result => result,
        }
    }
    pub(in crate::candidate_custody) fn finalize_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        accepted: bool,
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        self.finalize_snapshot_receipt(binding, snapshot, accepted, || {})
    }
    fn finalize_snapshot_receipt(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        accepted: bool,
        after_finalize: impl FnOnce(),
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        let mut finalized = false;
        let result = self.with_live_capture(binding, |owner| {
            match owner {
                Owner::Effect(owner) => owner.finalize_state_snapshot(snapshot, accepted),
                Owner::Loaned(owner) => owner.finalize_state_snapshot(snapshot, accepted),
                _ => Err("native snapshot owner differs"),
            }?;
            finalized = true;
            after_finalize();
            Ok(crate::plugin_worker::NativeSnapshotAction::Current)
        });
        match result {
            Err(_) if finalized => Ok(crate::plugin_worker::NativeSnapshotAction::SourceClosed),
            result => result,
        }
    }
    pub(in crate::candidate_custody) fn poll_live_native_snapshot(
        &self,
        binding: Binding,
        request: &NativeWorkerCapture,
    ) -> Result<crate::plugin_worker::NativeWorkerStateSnapshotProgress, &'static str> {
        self.snapshot_receipt(binding, request, || {})
    }
    fn snapshot_receipt(
        &self,
        binding: Binding,
        request: &NativeWorkerCapture,
        after_terminal: impl FnOnce(),
    ) -> Result<crate::plugin_worker::NativeWorkerStateSnapshotProgress, &'static str> {
        let mut consumed = false;
        let result = self.with_live_capture(binding, |owner| {
            let result = match owner {
                Owner::Effect(owner) => owner.poll_live_state_snapshot(request)?,
                Owner::Loaned(owner) => owner.poll_live_state_snapshot(request)?,
                _ => return Err("native snapshot owner differs"),
            };
            if matches!(
                result,
                crate::plugin_worker::NativeWorkerStateSnapshotProgress::Captured(_)
            ) {
                consumed = true;
                after_terminal();
            }
            Ok(result)
        });
        match result {
            Err(_) if consumed => {
                Ok(crate::plugin_worker::NativeWorkerStateSnapshotProgress::Discarded)
            }
            result => result,
        }
    }
    pub(in crate::candidate_custody) fn matches_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
    ) -> Result<bool, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.matches_live_state_snapshot(snapshot),
            Owner::Loaned(owner) => owner.matches_live_state_snapshot(snapshot),
            _ => Err("native snapshot owner differs"),
        })
    }
    pub(in crate::candidate_custody) fn matches_staged_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        candidate: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(owner) => owner.matches_staged_state_snapshot(snapshot, candidate),
            Owner::Loaned(owner) => owner.matches_staged_state_snapshot(snapshot, candidate),
            _ => Err("native snapshot owner differs"),
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
    // Capture and parameter flush use the same original quiescent-owner
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
                    cell.failure_stage = Some("native quiescent operation");
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
        if captured.is_none() {
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
    ) -> Result<crate::plugin_worker::NativeParameterPoll, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .drain_stopped_native_parameter_change(binding, request)?;
        match self.require_native_source_control_open() {
            Ok(()) => Ok(result),
            Err(reason) => result.source_closed(reason),
        }
    }
    pub(crate) fn matches_native_accepted_state(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        self.require_native_source_control_open()?;
        let matches = self
            .0
            .eligible
            .matches_native_accepted_state(binding, state)?;
        self.require_native_source_control_open()?;
        Ok(matches)
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
    pub(crate) fn native_source_parameter_values(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_worker::NativeParameterValues, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.native_parameter_values(binding)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn begin_native_parameter_change(
        &self,
        binding: Binding,
        id: u32,
        normalized: f64,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .begin_native_parameter_change(binding, id, normalized)?;
        Ok(result)
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
    ) -> Result<crate::plugin_worker::NativeParameterPoll, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .poll_native_parameter_change(binding, request)?;
        match self.require_native_source_control_open() {
            Ok(()) => Ok(result),
            Err(reason) => result.source_closed(reason),
        }
    }
    pub(crate) fn begin_native_controller_query(
        &self,
        binding: Binding,
        query: hex_plugin_host::protocol::ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .begin_native_controller_query(binding, query)?;
        Ok(result)
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
        match self.require_native_source_control_open() {
            Ok(()) => Ok(result),
            Err(reason) => result.source_closed(reason),
        }
    }
    pub(crate) fn drain_stopped_native_controller_query(
        &self,
        binding: Binding,
        request: &NativeWorkerControllerQuery,
    ) -> Result<bool, &'static str> {
        self.require_native_source_control_open()?;
        let drained = self
            .0
            .eligible
            .drain_stopped_native_controller_query(binding, request)?;
        if !drained {
            self.require_native_source_control_open()?;
        }
        Ok(drained)
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
    pub(crate) fn stage_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .stage_live_native_snapshot(binding, snapshot)?;
        if self.require_native_source_control_open().is_err() {
            Ok(crate::plugin_worker::NativeSnapshotAction::SourceClosed)
        } else {
            Ok(result)
        }
    }
    pub(crate) fn finalize_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        accepted: bool,
    ) -> Result<crate::plugin_worker::NativeSnapshotAction, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .finalize_live_native_snapshot(binding, snapshot, accepted)?;
        if self.require_native_source_control_open().is_err() {
            Ok(crate::plugin_worker::NativeSnapshotAction::SourceClosed)
        } else {
            Ok(result)
        }
    }
    pub(crate) fn poll_live_native_snapshot(
        &self,
        binding: Binding,
        request: &NativeWorkerCapture,
    ) -> Result<crate::plugin_worker::NativeWorkerStateSnapshotProgress, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .poll_live_native_snapshot(binding, request)?;
        match self.require_native_source_control_open() {
            Ok(()) => Ok(result),
            Err(reason) => result.source_closed(reason),
        }
    }
    pub(crate) fn matches_live_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
    ) -> Result<bool, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .matches_live_native_snapshot(binding, snapshot)?;
        self.require_native_source_control_open()?;
        Ok(result)
    }
    pub(crate) fn matches_staged_native_snapshot(
        &self,
        binding: Binding,
        snapshot: &crate::plugin_worker::NativeWorkerStateSnapshot,
        candidate: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .matches_staged_native_snapshot(binding, snapshot, candidate)?;
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
