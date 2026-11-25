//! Exact authored stage matching for a cross-track cut between owner-bound runtimes.
use crate::{NativeRuntimeChain, ProcessorChainOwner, ProcessorInstanceIdentity, RuntimeOwnership};

impl NativeRuntimeChain {
    /// Match the donor stage to the receiver's stage across arbitrary-length
    /// native chains when a single device moved between two tracks at any slot
    /// position while all remaining stages on both tracks match their prior
    /// identities and configurations.
    pub fn cross_track_moved_device_from(
        old_source: &Self,
        new_source: &Self,
        old_dest: &Self,
        new_dest: &Self,
    ) -> Option<ProcessorInstanceIdentity> {
        if old_source.len().checked_sub(1) != Some(new_source.len())
            || old_dest.len().checked_add(1) != Some(new_dest.len())
        {
            return None;
        }
        let sibling_unchanged = |a: &crate::RuntimeEntry, b: &crate::RuntimeEntry| {
            Self::same_runtime_identity(a, b)
                && a.bypassed == b.bypassed
                && a.input_gain == b.input_gain
                && a.output_gain == b.output_gain
                && a.matrix == b.matrix
                && a.configuration
                    .as_ref()
                    .zip(b.configuration.as_ref())
                    .is_some_and(|(x, y)| {
                        std::sync::Arc::ptr_eq(x, y) || x.processing_eq_on_callback(y)
                    })
        };
        // Locate the single removed stage in `old_source`.
        let src_pos = new_source
            .entries
            .iter()
            .zip(&old_source.entries)
            .position(|(next, prev)| !Self::same_runtime_identity(next, prev))
            .unwrap_or(new_source.len());
        let old_entry = old_source.entries.get(src_pos)?;
        let surviving_source = old_source
            .entries
            .iter()
            .take(src_pos)
            .chain(old_source.entries.iter().skip(src_pos + 1));
        if !new_source
            .entries
            .iter()
            .zip(surviving_source)
            .all(|(a, b)| sibling_unchanged(a, b))
            || !new_source.can_inherit_from(old_source)
        {
            return None;
        }
        // Locate the single inserted stage in `new_dest`.
        let dst_pos = old_dest
            .entries
            .iter()
            .zip(&new_dest.entries)
            .position(|(prev, next)| !Self::same_runtime_identity(prev, next))
            .unwrap_or(old_dest.len());
        let new_entry = new_dest.entries.get(dst_pos)?;
        let surviving_dest = new_dest
            .entries
            .iter()
            .take(dst_pos)
            .chain(new_dest.entries.iter().skip(dst_pos + 1));
        if !old_dest
            .entries
            .iter()
            .zip(surviving_dest)
            .all(|(a, b)| sibling_unchanged(a, b))
            || !new_dest.can_inherit_from(old_dest)
        {
            return None;
        }
        Self::moved_native_entry_identity(new_entry, old_entry)
    }

    /// Match the donor stage to the receiver's final stage across chains of any
    /// length when the moved stage occupies the tail slot.
    pub fn cross_track_native_identity_from(
        &self,
        old: &Self,
    ) -> Option<ProcessorInstanceIdentity> {
        let next = self.entries.last()?;
        let prev = old.entries.last()?;
        Self::moved_native_entry_identity(next, prev)
    }

    fn moved_native_entry_identity(
        next: &crate::RuntimeEntry,
        old: &crate::RuntimeEntry,
    ) -> Option<ProcessorInstanceIdentity> {
        let (ProcessorChainOwner::Track(to), ProcessorChainOwner::Track(from)) =
            (next.owner, old.owner)
        else {
            return None;
        };
        let settled = |entry: &crate::RuntimeEntry| {
            !entry.bypassed
                && !entry.removal_bypassing
                && !entry.automation_frozen
                && !entry.committed_bypass_ramp.is_active()
                && !entry.committed_matrix_ramp.is_active()
                && !entry.committed_output_gain_ramp.active()
                && entry.committed_transient_output_gain
                    == crate::host_output_gain_ramp::TransientGain::Follow
                && !matches!(entry.runtime, RuntimeOwnership::Foreign(_))
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

    #[test]
    fn cross_track_moved_device_matches_arbitrary_positions_in_multi_device_chains() {
        let probe = Arc::new(Probe::default());
        let device =
            |track, id| instance(ProcessorChainOwner::Track(track), id, 48_000, 0, &probe).unwrap();
        let old_src =
            NativeRuntimeChain::new_prepared(vec![device(1, 10), device(1, 11), device(1, 12)])
                .unwrap();
        let new_src = NativeRuntimeChain::new_prepared(vec![device(1, 10), device(1, 12)]).unwrap();
        let old_dst = NativeRuntimeChain::new_prepared(vec![device(2, 20), device(2, 21)]).unwrap();
        let new_dst =
            NativeRuntimeChain::new_prepared(vec![device(2, 20), device(2, 11), device(2, 21)])
                .unwrap();
        let donor = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 11).unwrap();
        assert_eq!(
            NativeRuntimeChain::cross_track_moved_device_from(
                &old_src, &new_src, &old_dst, &new_dst
            ),
            Some(donor)
        );
    }
}
