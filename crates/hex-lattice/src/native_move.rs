//! Explicit cross-Track history permission; ordinary identities remain owner-bound.
use crate::{NativeRuntimeChain, ProcessorChainOwner, ProcessorInstanceIdentity, RuntimeOwnership};
use hex_lattice_contract::DeviceContractError;

impl NativeRuntimeChain {
    /// Match the zero-latency donor stage to the receiver's final stage, with
    /// at most one retained prefix stage across the two chains.
    /// The host must independently approve the provider's owner change, any
    /// unchanged prefix, both route cuts and exclusive donor retirement.
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

    /// Move history once after the same preflight. For an approved legacy
    /// provider, the host must prove that the two prepared runtime pairs are
    /// interchangeable. Exchange the pairs so moving a large running effect
    /// adds no ring copy to the cut. The old chain retires with the fresh pair.
    pub fn transfer_cross_track_native_from(
        &mut self,
        old: &mut Self,
    ) -> Result<(), DeviceContractError> {
        if self.cross_track_native_identity_from(old).is_none() {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let (Some(next), Some(old)) = (self.entries.last_mut(), old.entries.last_mut()) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if matches!(next.runtime, RuntimeOwnership::Legacy { .. }) {
            // The preflight already proved that both entries are Legacy.
            std::mem::swap(&mut next.runtime, &mut old.runtime);
        } else {
            next.runtime.transfer_history_from(&mut old.runtime);
        }
        Ok(())
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
    fn cross_track_move_exchanges_the_legacy_runtime_pairs() {
        let probe = Arc::new(Probe::default());
        let device =
            |track, id| instance(ProcessorChainOwner::Track(track), id, 48_000, 0, &probe).unwrap();
        let pair = |entry: &crate::RuntimeEntry| {
            let RuntimeOwnership::Legacy { primary, scratch } = &entry.runtime else {
                panic!("legacy pair");
            };
            [
                std::ptr::from_ref(primary.as_ref()).cast::<()>(),
                std::ptr::from_ref(scratch.as_ref()).cast::<()>(),
            ]
        };
        let mut old = NativeRuntimeChain::new_prepared(vec![device(1, 4)]).unwrap();
        let mut next = NativeRuntimeChain::new_prepared(vec![device(2, 5), device(2, 4)]).unwrap();
        let running = pair(&old.entries[0]);
        let prefix = pair(&next.entries[0]);
        let fresh = pair(&next.entries[1]);
        next.transfer_cross_track_native_from(&mut old).unwrap();
        assert_eq!(pair(&next.entries[0]), prefix);
        assert_eq!(pair(&next.entries[1]), running);
        assert_eq!(pair(&old.entries[0]), fresh);

        old.transfer_cross_track_native_from(&mut next).unwrap();
        assert_eq!(pair(&next.entries[0]), prefix);
        assert_eq!(pair(&next.entries[1]), fresh);
        assert_eq!(pair(&old.entries[0]), running);
    }
}
