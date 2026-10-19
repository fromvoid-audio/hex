//! Nonextracting concrete producer route into the actual preparation custodian.
//! Numeric local allowances never certify global union, source freshness or native
//! publication. Those remain mandatory on the sealed same-worker host route.
use super::*;
use crate::native_device_resources::{
    OriginalResourceBankPlan, OriginalResourcePlanReservation, ResourcePreparationError,
};
impl AudioCandidatePreparation {
    /// One fixed target loan from compiler preflight through runtime sealing.
    /// This is private construction, not a Service source/publication binding.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Service consumer under construction")
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "closed actual captured and producer input operation"
    )]
    pub(in crate::audio_candidate) fn prepare_original_sampler_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        producer: &OriginalResourceBankPlan<'_>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &crate::AudioRootStorageLedger<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        reservation: crate::AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        if self.failed_incomplete()
            || self.cleaning
            || self.complete
            || !self.original_resource_attempted
            || self.original_resource_refused
            || self.bank.is_some()
            || self.token.is_some()
            || self.owner.is_some()
            || !self.sources.is_empty()
            || self.resident_draft.capacity() != 0
            || self.custody.is_none()
            || self.draft.is_none()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if reservation.max_retained_bytes > self.reservation.max_prepared_data_bytes
            || reservation.max_construction_peak_bytes
                > self.reservation.max_construction_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        // Existing draft/runtime checks bound the actual new mutable buffers,
        // group/archive/journal and split wrapper by the unchanged runtime cap.
        // Keep the still-live persistent host wrapper/backing separately charged
        // while compiler construction and its temporary ledgers overlap it.
        let groups = crate::session_playback::candidate_group_reservation(
            target.tracks().len(),
            target.return_tracks().len(),
        )?;
        let runtime_metadata = groups
            .checked_mul(
                std::mem::size_of::<Group>()
                    + std::mem::size_of::<groups::TransferRow>()
                    + std::mem::size_of::<groups::ArchivedOrigin>(),
            )
            .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
            .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
            .and_then(|n| n.checked_add(claimed::reserved_state_bytes()))
            .ok_or(AudioStorageError::Overflow)?;
        let extra = self
            .collection_bytes()?
            .checked_add(runtime_metadata)
            .and_then(|n| n.checked_add(crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
            .ok_or(AudioStorageError::Overflow)?;
        let mut compiler = reservation;
        compiler.max_retained_bytes = compiler
            .max_retained_bytes
            .checked_sub(extra)
            .ok_or(AudioStorageError::ReservationExceeded)?;
        compiler.max_construction_peak_bytes = compiler
            .max_construction_peak_bytes
            .checked_sub(extra)
            .ok_or(AudioStorageError::ReservationExceeded)?;
        let custody = self
            .custody
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        // The actual closed compiler checks BOTH reduced ceilings and normal
        // mandatory-input closure before constructing the first new owner.
        self.token = Some(
            custody.compile_original_sampler_revision_with_producer_on_control(
                target,
                Some(producer),
                editor,
                previous,
                original,
                transforms,
                compiler,
            )?,
        );
        let draft = self
            .draft
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        draft.prepare_on_control(
            custody,
            self.token
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?,
        )?;
        draft.finish_on_control(&mut self.custody, &mut self.token, &mut self.owner)?;
        self.owner
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?
            .seal_sampler_constructor_on_control(
                target,
                Some(producer),
                editor,
                previous,
                original,
                transforms,
                reservation,
            )?;
        Ok(())
    }

    pub(in crate::audio_candidate) fn install_original_resources_on_control(
        &mut self,
        plan: &OriginalResourceBankPlan<'_>,
        allowance: OriginalResourcePlanReservation,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<(), ResourcePreparationError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.original_resource_attempted
            || self.owner.is_some()
            || self.bank.is_some()
            || self.token.is_some()
            || self.resident_draft.capacity() != 0
            || self
                .custody
                .as_ref()
                .is_none_or(|custody| !custody.original_resource_destination_vacant())
        {
            return Err(AudioStorageError::SourceMismatch.into());
        }
        // These are genuine ceilings on this already-reserved cell, not a
        // scalar admission permit or a substitute for the enclosing prequote.
        let base = self.collection_bytes()?;
        let retained = base
            .checked_add(allowance.max_retained_data_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        let peak = base
            .checked_add(allowance.max_construction_peak_bytes)
            .and_then(|bytes| bytes.checked_add(self.reservation.caller_construction_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if retained > self.reservation.max_prepared_data_bytes
            || peak > self.reservation.max_construction_data_bytes
            || allowance.inputs.max_prepared_data_bytes > allowance.max_retained_data_bytes
            || allowance.inputs.max_construction_data_bytes > allowance.max_construction_peak_bytes
            || allowance.max_preparation_metadata_bytes > allowance.max_construction_peak_bytes
            || allowance.catalog.metadata_bytes > allowance.max_construction_peak_bytes
        {
            return Err(AudioStorageError::ReservationExceeded.into());
        }
        self.original_resource_attempted = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.custody
                .as_mut()
                .ok_or(AudioStorageError::SourceMismatch)?
                .prepare_original_resources_on_control(plan, allowance, cancel)
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.original_resource_refused = true;
                Err(error)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.failed = true;
                Err(AudioStorageError::UnknownStorage.into())
            }
        }
    }
}

#[cfg(test)]
mod tests;
