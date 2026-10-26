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
impl Configuration {
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
    if !worker.owns_live_state_snapshot_on_control(snapshot)
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
