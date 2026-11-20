//! Explicit cross-Track history permission; ordinary identities remain owner-bound.
use crate::{NativeRuntimeChain, ProcessorChainOwner, ProcessorInstanceIdentity, RuntimeOwnership};
use hex_lattice_contract::DeviceContractError;

impl NativeRuntimeChain {
    /// Match one unchanged zero-latency native processor under another Track.
    /// The host must independently approve the provider's owner change, both
    /// route cuts and exclusive donor retirement. This is not route admission.
    pub fn cross_track_native_identity_from(
        &self,
        old: &Self,
    ) -> Option<ProcessorInstanceIdentity> {
        let ([next], [old]) = (self.entries.as_slice(), old.entries.as_slice()) else {
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

    /// Move history once after the same preflight and before retiring the donor.
    /// No runtime is destroyed here; both allocations stay in host custody.
    pub fn transfer_cross_track_native_from(
        &mut self,
        old: &mut Self,
    ) -> Result<(), DeviceContractError> {
        if self.cross_track_native_identity_from(old).is_none() {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let (Some(next), Some(old)) = (self.entries.first_mut(), old.entries.first_mut()) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        next.runtime.transfer_history_from(&mut old.runtime);
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
}
