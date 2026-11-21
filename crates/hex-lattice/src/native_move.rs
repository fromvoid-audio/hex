//! Exact authored stage matching for a cross-track cut between owner-bound runtimes.
use crate::{NativeRuntimeChain, ProcessorChainOwner, ProcessorInstanceIdentity, RuntimeOwnership};

impl NativeRuntimeChain {
    /// Match the zero-latency donor stage to the receiver's final stage, with
    /// at most one retained prefix stage across the two chains.
    /// The host separately proves the unchanged prefix and the two route
    /// fades. The caller uses the donor identity to end its residual signal,
    /// while the receiver keeps the fresh runtime prepared under its owner.
    pub fn cross_track_native_identity_from(
        &self,
        old: &Self,
    ) -> Option<ProcessorInstanceIdentity> {
        let (([next] | [_, next], [old]) | ([next], [_, old])) =
            (self.entries.as_slice(), old.entries.as_slice())
        else {
            return None;
        };
        let (ProcessorChainOwner::Track(to), ProcessorChainOwner::Track(from)) =
            (next.owner, old.owner)
        else {
            return None;
        };
        let settled = |entry: &crate::RuntimeEntry| {
            !entry.bypassed
                && !entry.removal_bypassing
                && !entry.automation_frozen
                && entry.automation.is_empty()
                && entry.modulation.is_none()
                && entry.algorithmic_latency_frames == Some(0)
                && !entry.committed_bypass_ramp.is_active()
                && !entry.committed_matrix_ramp.is_active()
                && !entry.committed_output_gain_ramp.active()
                && entry.committed_transient_output_gain
                    == crate::host_output_gain_ramp::TransientGain::Follow
                && !matches!(
                    entry.runtime,
                    RuntimeOwnership::Memoryless | RuntimeOwnership::Foreign(_)
                )
        };
        (to != from
            && next.device_id == old.device_id
            && next.device_type == old.device_type
            && next.runtime_version == old.runtime_version
            && next.sample_rate == old.sample_rate
            && next.input_gain == old.input_gain
            && next.output_gain == old.output_gain
            && next.matrix == old.matrix
            && settled(next)
            && settled(old)
            && next
                .configuration
                .as_ref()
                .zip(old.configuration.as_ref())
                .is_some_and(|(a, b)| {
                    std::sync::Arc::ptr_eq(a, b) || a.processing_eq_on_callback(b)
                })
            && next.runtime.can_inherit_from(&old.runtime))
        .then(|| ProcessorInstanceIdentity::new(old.owner, old.device_id).ok())
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance::transition_fixture::{Probe, instance};
    use std::sync::Arc;

    #[test]
    fn owner_change_is_explicit_and_requires_exact_native_configuration() {
        let probe = Arc::new(Probe::default());
        let chain = |owner, id, mode| {
            NativeRuntimeChain::new_prepared(vec![
                instance(ProcessorChainOwner::Track(owner), id, 48_000, mode, &probe).unwrap(),
            ])
            .unwrap()
        };
        let old = chain(1, 4, 0);
        let mut moved = chain(2, 4, 0);
        assert!(!NativeRuntimeChain::same_runtime_identity(
            &moved.entries[0],
            &old.entries[0]
        ));
        assert!(moved.cross_track_native_identity_from(&old).is_some());
        assert!(
            chain(1, 4, 0)
                .cross_track_native_identity_from(&old)
                .is_none()
        );
        assert!(
            chain(2, 5, 0)
                .cross_track_native_identity_from(&old)
                .is_none()
        );
        assert!(
            chain(2, 4, 1)
                .cross_track_native_identity_from(&old)
                .is_none()
        );
        moved.entries[0].bypassed = true;
        assert!(moved.cross_track_native_identity_from(&old).is_none());
    }

    #[test]
    fn cross_track_cut_identifies_the_donor_with_one_receiver_prefix() {
        let probe = Arc::new(Probe::default());
        let device =
            |track, id| instance(ProcessorChainOwner::Track(track), id, 48_000, 0, &probe).unwrap();
        let old = NativeRuntimeChain::new_prepared(vec![device(1, 4)]).unwrap();
        let next = NativeRuntimeChain::new_prepared(vec![device(2, 5), device(2, 4)]).unwrap();
        let donor = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 4).unwrap();
        assert_eq!(next.cross_track_native_identity_from(&old), Some(donor));
    }
}
