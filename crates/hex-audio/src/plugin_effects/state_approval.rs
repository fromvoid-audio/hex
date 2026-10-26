//! Captured-state matching through the existing original effect/control owner.
//! One accepted and one pending configuration are bounded compiler permissions,
//! not canonical history or a second source lifecycle. The caller finalizes the
//! pending operation only with its existing canonical gesture terminal. Both
//! slots are inline in Configuration, so source/endpoint DATA capacity cannot
//! grow after admission. No source-side allocation occurs when staging. A
//! pending permission freezes further host input until its exact terminal;
//! cancellation never pretends to reverse input already consumed by the SDK.
use super::*;
use crate::plugin_worker::NativeWorkerStateSnapshot;

#[derive(Clone)]
pub(super) struct ApprovedState {
    ordinal: u64,
    proof: hex_lattice::ForeignStateApproval,
}
/// Accepted configuration DATA travelling with Root's unique quiescent source.
/// Its old worker anchor has been removed after actual quiescence. There is no
/// public constructor, native owner, pending gesture or graph permission here.
pub(crate) struct SuspendedState(ApprovedState);
impl ApprovedState {
    pub(super) fn suspend(self) -> SuspendedState {
        SuspendedState(Self {
            ordinal: self.ordinal,
            proof: self.proof.detached_after_quiescence_on_control(),
        })
    }
}
impl NativeEffect {
    pub(crate) fn has_pending_state_approval(&self) -> bool {
        self.callback.configuration.pending_state.is_some()
    }
    /// Root calls only after its same retained native lease has resumed and
    /// constructed a fresh paired worker/endpoint. Caller DATA cannot enter
    /// this path: SuspendedState is minted by the original effect's quiescence.
    pub(crate) fn restore_state_approval_after_resume(
        &mut self,
        state: &SuspendedState,
    ) -> Result<(), &'static str> {
        if !self.worker.transport_is_live_on_control()
            || !self.worker.retains_original_source_on_control()
            || self.callback.configuration.approval_count() != 0
        {
            return Err("native resumed state source differs");
        }
        let proof = state
            .0
            .proof
            .after_original_resume_on_control(self.worker.state_source_on_control(), self.binding())
            .map_err(|_| "native resumed state binding differs")?;
        self.callback.configuration.accepted_state = Some(ApprovedState {
            ordinal: state.0.ordinal,
            proof,
        });
        Ok(())
    }
}
impl Configuration {
    /// Registered canonical baseline only, never pending candidate bytes or
    /// proof that the plugin's evolving DSP still equals an earlier capture.
    fn matches_accepted_state(&self, state: &NativeDeviceState) -> Result<bool, &'static str> {
        if self.pending_state.is_some() {
            return Err("native state approval is pending");
        }
        Ok(match self.accepted_state.as_ref() {
            Some(accepted) => accepted.proof.matches_state(state),
            None => self.state.processing_eq(state),
        })
    }
    pub(super) fn approval_count(&self) -> usize {
        usize::from(self.accepted_state.is_some()) + usize::from(self.pending_state.is_some())
    }
    pub(super) fn state_approval(
        &self,
        state: &NativeDeviceState,
    ) -> Option<&hex_lattice::ForeignStateApproval> {
        [&self.pending_state, &self.accepted_state]
            .into_iter()
            .flatten()
            .map(|approved| &approved.proof)
            .find(|proof| proof.matches_state(state))
    }
    pub(super) fn approves_factory(
        &self,
        factory: &hex_lattice::PreparedProcessorFactory,
        state: &NativeDeviceState,
    ) -> bool {
        factory.foreign_processor() == Some(self.prepared)
            && match factory.foreign_state_approval() {
                Some(proof) => self.state_approval(state).is_some_and(|registered| {
                    proof.same_source(registered) && proof.matches_state(state)
                }),
                None => state.processing_eq(&self.state),
            }
    }
}

impl NativeEffectReference<'_> {
    pub(crate) fn matches_accepted_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        self.configuration().matches_accepted_state(state)
    }
}

pub(super) fn stage(
    worker: &NativeWorker,
    configuration: &mut Configuration,
    snapshot: &NativeWorkerStateSnapshot,
    current_data_bytes: usize,
    max_data_bytes: usize,
) -> Result<(), &'static str> {
    if configuration.pending_state.is_some() {
        return Err("native state approval is already pending");
    }
    // A source capture must retain the original plugin code/class identity.
    let original = hex_plugin_state::decode(&configuration.state)
        .map_err(|_| "native original state is invalid")?;
    let captured = hex_plugin_state::decode(snapshot.state())
        .map_err(|_| "native captured state is invalid")?;
    if original.identity != captured.identity {
        return Err("native captured plugin identity differs");
    }
    if current_data_bytes
        .checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES)
        .is_none_or(|bytes| bytes > max_data_bytes)
    {
        return Err("native state approval data reservation exceeded");
    }
    let proof = worker.state_snapshot_approval_on_control(snapshot)?;
    configuration.pending_state = Some(ApprovedState {
        ordinal: snapshot.ordinal(),
        proof,
    });
    Ok(())
}
pub(super) fn finalize(
    worker: &NativeWorker,
    configuration: &mut Configuration,
    snapshot: &NativeWorkerStateSnapshot,
    accepted: bool,
) -> Result<(), &'static str> {
    if !worker.owns_state_snapshot_on_control(snapshot)
        || configuration.pending_state.as_ref().is_none_or(|pending| {
            pending.ordinal != snapshot.ordinal() || !pending.proof.matches_state(snapshot.state())
        })
    {
        return Err("native state approval source or operation differs");
    }
    let pending = configuration.pending_state.take();
    if accepted {
        configuration.accepted_state = pending;
    }
    // Refused/superseded weak handles drop only on control; inline DATA capacity stays reserved.
    Ok(())
}

/// Query the exact still-staged capture without promoting or consuming it.
/// The worker's live-source and latest-capture checks remain part of admission;
/// matching candidate bytes alone cannot identify a native operation.
pub(super) fn matches_staged(
    worker: &NativeWorker,
    configuration: &Configuration,
    snapshot: &NativeWorkerStateSnapshot,
    candidate: &NativeDeviceState,
) -> Result<bool, &'static str> {
    if !worker.matches_live_state_snapshot_on_control(snapshot)? {
        return Ok(false);
    }
    Ok(configuration.pending_state.as_ref().is_some_and(|pending| {
        pending.ordinal == snapshot.ordinal()
            && pending.proof.matches_state(snapshot.state())
            && pending.proof.matches_state(candidate)
    }))
}
