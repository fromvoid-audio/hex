//! Scoped constructor DATA and actual shared-handle closure. This is not the
//! public scalar ledger's completeness flag, native admission or RSS/OS bytes.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageLedgerReservation,
    audio_storage::{Location, Owner},
};
/// Informational totals. Only private retained constructor custody can use the
/// corresponding stored declaration; copying this summary grants no authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateConstructorSummary {
    pub data: AudioRootStorageSummary,
    pub owned_runtime_bytes: usize,
    /// Conservative mutable storage retained by the embedding native runtime.
    /// It reduces this runtime's allowance; it is not a DATA/custody proof.
    pub external_runtime_bytes: usize,
    pub reserved_split_bytes: usize,
    pub shared_resource_groups: usize,
    pub shared_resource_handles: usize,
    /// Actual reserved runtime member plus original archive rows. Vacant
    /// locations count too; their capacity remains through report consumption.
    pub member_rows: usize,
    /// Actual journal backing, later moved into the genuine transfer manifest.
    pub transfer_rows: usize,
}
impl AudioCandidateOwner {
    /// Inspect and seal only a fresh, closed constructor. Every DATA declaration
    /// comes from that same actual custody. Arc counts merely refuse additional
    /// caller aliases; they never supply provenance or erase opaque resources.
    /// Refusal leaves all original owners/bank/runtime in this exact wrapper.
    pub fn seal_constructor_on_control(
        &mut self,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        self.seal_constructor_with_runtime_storage_on_control(reservation, 0)
    }
    /// Add the embedding owner's measured mutable wrapper/workspace capacity to
    /// the same 32 MiB runtime allowance before any split. The embedding owner
    /// must also account that actual DATA/peak storage in its own ledger. This
    /// conservative input grants no provenance or completeness authority.
    pub fn seal_constructor_with_runtime_storage_on_control(
        &mut self,
        reservation: AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        self.inner.constructor_data = None;
        if self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.runtime_receipt.is_some()
            || self.inner.first_panic.is_some()
            || self.inner.receipt_taken
            || self.inner.recovered_inputs.is_some()
            || self.inner.inputs_drop_pending
            || self.inner.inputs_disposed
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.inner.validate_groups()?;
        self.inner.validate_fresh_origins()?;
        let summary = self.inspect_constructor_storage(reservation, external_runtime_bytes)?;
        self.inner.constructor_data = Some(summary);
        Ok(summary)
    }
    fn inspect_constructor_storage(
        &self,
        reservation: AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.mutate(|l| {
            l.root(self, crate::AudioStorageRole::Current)?;
            self.inner.custody.visit_audio_storage(l)
        })?;
        let before = ledger.summary()?.data_bytes;
        ledger.mutate(|l| self.inner.visit_fresh_storage(l))?;
        let data = ledger
            .summary()?
            .require_known_storage()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        ledger.validate_closed_shared_handles()?;
        let owned_runtime_bytes = data
            .data_bytes
            .checked_sub(before)
            .ok_or(AudioStorageError::Overflow)?;
        let reserved_split_bytes = claimed::reserved_state_bytes();
        let owned_runtime_bytes = owned_runtime_bytes
            .checked_add(reserved_split_bytes)
            .and_then(|bytes| bytes.checked_add(external_runtime_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if owned_runtime_bytes > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(AudioStorageError::ReservationExceeded);
        }
        // Both the actual current ledger buffers and future split wrapper are
        // simultaneously reserved before any publication ownership movement.
        if data
            .retained_bytes
            .checked_add(reserved_split_bytes)
            .ok_or(AudioStorageError::Overflow)?
            > reservation.max_retained_bytes
            || data
                .construction_peak_bytes
                .checked_add(reserved_split_bytes)
                .ok_or(AudioStorageError::Overflow)?
                > reservation.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        let (shared_resource_groups, shared_resource_handles) = ledger.shared_resource_counts();
        Ok(AudioCandidateConstructorSummary {
            data,
            owned_runtime_bytes,
            external_runtime_bytes,
            reserved_split_bytes,
            shared_resource_groups,
            shared_resource_handles,
            member_rows: self
                .inner
                .groups
                .capacity()
                .checked_add(self.inner.archive.groups.capacity())
                .ok_or(AudioStorageError::Overflow)?,
            transfer_rows: self.inner.journal.capacity(),
        })
    }
}
impl CandidateState {
    pub(super) fn validate_fresh_origins(&self) -> Result<(), AudioStorageError> {
        // A constructor incarnation never replenishes an emptied owning
        // location with a fresh owner; native sealing cannot adopt a lineage.
        if self.groups.iter().enumerate().any(|(member, group)| {
            group.closed
                || group.origin
                    != group.present.then_some(groups::Origin {
                        runtime: self.identity,
                        member,
                    })
        }) || self.archive.original_disposed
            || self.archive.groups.iter().any(|g| g.closed)
            || !self.journal.is_empty()
            || self
                .runtime
                .as_ref()
                .is_none_or(|r| !r.playback.candidate_constructor_is_fresh())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }

    fn visit_fresh_storage<'a>(
        &'a self,
        l: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            identity: _,
            custody: _,
            token: _,
            runtime,
            groups,
            journal,
            archive,
            runtime_receipt: _,
            phase: _,
            first_panic: _,
            receipt_taken: _,
            sample_rate: _,
            recovered_inputs: _,
            inputs_drop_pending: _,
            inputs_disposed: _,
            constructor_data: _,
        } = self;
        l.bytes(
            Owner::exclusive(self),
            Location("candidate-state-box", 0),
            std::mem::size_of::<Self>(),
        )?;
        l.vec(groups)?;
        l.vec(journal)?;
        let groups::Archive {
            runtime: _,
            groups,
            original_disposed: _,
        } = archive;
        l.vec(groups)?;
        let runtime = runtime.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
        let RuntimeOwner {
            playback,
            sampler_sources,
            #[cfg(any(test, feature = "instrument-test-support"))]
            probe,
        } = runtime.as_ref();
        #[cfg(any(test, feature = "instrument-test-support"))]
        if probe.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        l.bytes(
            Owner::exclusive(runtime),
            Location("runtime-owner-box", 0),
            std::mem::size_of::<RuntimeOwner>(),
        )?;
        if let Some((sources, handle)) = sampler_sources {
            l.shared(
                sources.arc(),
                sources.parts(),
                Owner::exclusive(sources),
                *handle,
                None,
            )?;
            sources.visit(l)?;
        }
        playback.visit_fresh_candidate_storage(l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 4096,
            handle_rows: 2048,
            root_rows: 4,
            max_retained_bytes: 64 * 1024 * 1024,
            max_construction_peak_bytes: 64 * 1024 * 1024,
        }
    }
    #[test]
    fn actual_constructor_data_refuses_external_root_handle_and_preserves_bank_on_caps() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let extra = owner
            .inner
            .runtime
            .as_ref()
            .unwrap()
            .playback
            .prepared_revision()
            .unwrap()
            .clone();
        assert_eq!(
            owner.seal_constructor_on_control(reservation()),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(owner.inner.constructor_data.is_none());
        drop(extra);
        let weak = std::sync::Arc::downgrade(
            owner
                .inner
                .runtime
                .as_ref()
                .unwrap()
                .playback
                .prepared_revision()
                .unwrap(),
        );
        assert_eq!(
            owner.seal_constructor_on_control(reservation()),
            Err(AudioStorageError::UnknownStorage)
        );
        drop(weak);
        let data = owner.seal_constructor_on_control(reservation()).unwrap();
        assert!(data.owned_runtime_bytes > std::mem::size_of::<crate::SessionPlayback>());
        assert!(
            data.shared_resource_groups > 0
                && data.shared_resource_handles >= data.shared_resource_groups
        );
        assert_eq!(data.data.unknown_storage, 0);
        assert!(data.data.require_complete().is_err());
        // Embedding runtime metadata reduces the existing allowance, without
        // changing graph DATA or turning the scalar ledger into authority.
        let remaining = crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES - data.owned_runtime_bytes;
        let capped = owner
            .seal_constructor_with_runtime_storage_on_control(reservation(), remaining)
            .unwrap();
        assert_eq!(
            capped.owned_runtime_bytes,
            crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
        );
        assert_eq!(capped.external_runtime_bytes, remaining);
        assert_eq!(capped.data, data.data);
        assert_eq!(
            owner.seal_constructor_with_runtime_storage_on_control(reservation(), remaining + 1),
            Err(AudioStorageError::ReservationExceeded)
        );
        assert_eq!(
            owner.seal_constructor_with_runtime_storage_on_control(reservation(), usize::MAX),
            Err(AudioStorageError::Overflow)
        );
        assert_eq!(owner.phase(), AudioCandidateDisposalPhase::Owned);
        let mut too_small = reservation();
        too_small.max_retained_bytes = data.data.retained_bytes;
        assert_eq!(
            owner.seal_constructor_on_control(too_small),
            Err(AudioStorageError::ReservationExceeded)
        );
        assert!(owner.inner.constructor_data.is_none());
        assert_eq!(owner.phase(), AudioCandidateDisposalPhase::Owned);
        owner.dispose_on_control().unwrap();
        let bank = owner.recover_inputs_on_control().unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
    }
}
