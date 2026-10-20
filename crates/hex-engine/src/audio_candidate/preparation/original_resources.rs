//! Nonextracting concrete producer route into the actual preparation custodian.
//! Numeric local allowances never certify global union, source freshness or native
//! publication. Those remain mandatory on the sealed same-worker host route.
use super::*;
use crate::native_device_resources::{
    OriginalResourceBankPlan, OriginalResourcePlanReservation, ResourcePreparationError,
};
// Engine-private, consumed once inside the closed same-parent driver. Neither
// scalar bounds nor correlation addresses grant source/storage authority. The
// mandatory immutable inventory produced `compiler` before any owner birth.
// No references survive into the selected parent's mutable reborrow.
pub(crate) struct OriginalSamplerConstruction {
    compiler: crate::audio_preparation::OriginalSamplerConstruction,
    compiler_reservation: crate::AudioStorageLedgerReservation,
    cell_address: usize,
    target_address: usize,
    purpose: ConstructorPurpose,
    retire_after_native_guards: bool,
}
impl AudioCandidatePreparation {
    pub(crate) fn original_sampler_host_custody(
        &self,
    ) -> Result<&AudioPreparationCustody, AudioStorageError> {
        self.custody
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "same mandatory closed host inputs"
    )]
    pub(crate) fn inspect_native_sampler_seal_in_host(
        &self,
        host: &crate::sampler_host_fields::OriginalSamplerConstructionHost<'_, '_>,
        resident: &dyn crate::sampler_host_fields::OriginalSamplerResidentFields,
        producer: &OriginalResourceBankPlan<'_>,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &crate::AudioRootStorageLedger<'_>,
        writers: &crate::audio_transform::OriginalSamplerTransaction<'_>,
        reservation: crate::AudioStorageLedgerReservation,
    ) -> Result<super::super::storage::OriginalSamplerSealInspection, AudioStorageError> {
        let inventory = writers.inventory();
        self.owner
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .inspect_sampler_constructor_with_inventory_on_control(
                Some(host),
                host.source.payload.project(),
                Some(resident),
                Some(producer),
                resident.editor(),
                previous,
                original,
                &inventory,
                reservation,
                ConstructorPurpose::NativeFresh,
            )
    }

    pub(in crate::audio_candidate) fn install_native_sampler_seal_in_host(
        &mut self,
        inspection: super::super::storage::OriginalSamplerSealInspection,
    ) -> Result<bool, AudioStorageError> {
        Ok(self
            .owner
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?
            .install_sampler_constructor_inspection_on_control(inspection)?
            .is_some())
    }

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
        self.prepare_original_sampler_for_purpose_on_control(
            target,
            producer,
            editor,
            previous,
            original,
            transforms,
            reservation,
            ConstructorPurpose::NativeFresh,
        )
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "same closed actual inputs and private birth purpose"
    )]
    pub(in crate::audio_candidate) fn prepare_original_sampler_for_purpose_on_control(
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
        purpose: ConstructorPurpose,
    ) -> Result<(), AudioStorageError> {
        self.prepare_original_sampler_with_writers_on_control(
            target,
            producer,
            editor,
            previous,
            original,
            transforms,
            reservation,
            purpose,
            None,
            None,
        )
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "same closed actual inputs and private birth purpose"
    )]
    pub(in crate::audio_candidate) fn prepare_original_sampler_with_writers_on_control(
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
        purpose: ConstructorPurpose,
        mut writers: Option<&mut crate::audio_transform::OriginalSamplerTransaction<'_>>,
        resident: Option<&dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
    ) -> Result<(), AudioStorageError> {
        if let Some(transaction) = writers.as_deref_mut() {
            if !std::ptr::eq(transaction.preparation(), transforms) {
                return Err(AudioStorageError::SourceMismatch);
            }
            // ALL temporary readonly ledger/projection loans end in inspect.
            let construction = self.inspect_original_sampler_with_writers_on_control(
                None,
                target,
                producer,
                resident,
                editor,
                previous,
                original,
                transaction,
                reservation,
                purpose,
            )?;
            self.construct_original_sampler_with_writers_on_control(
                target,
                previous,
                transaction,
                construction,
            )?;
        } else {
            let compiler = self.original_sampler_compiler_reservation(target, reservation)?;
            let custody = self
                .custody
                .as_mut()
                .ok_or(AudioStorageError::SourceMismatch)?;
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
            self.finish_original_sampler_runtime_on_control(purpose)?;
        }
        let owner = self
            .owner
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if let Some(transaction) = writers.as_deref() {
            owner.seal_sampler_constructor_with_writers_on_control(
                target,
                resident,
                Some(producer),
                editor,
                previous,
                original,
                transaction,
                reservation,
                purpose,
            )?;
        } else {
            owner.seal_sampler_constructor_for_purpose_on_control(
                target,
                Some(producer),
                editor,
                previous,
                original,
                transforms,
                reservation,
                purpose,
            )?;
        }
        Ok(())
    }

    fn original_sampler_compiler_reservation(
        &self,
        target: &crate::SharedMetadata<hex_project::Project>,
        reservation: crate::AudioStorageLedgerReservation,
    ) -> Result<crate::AudioStorageLedgerReservation, AudioStorageError> {
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
        let shell_bytes = groups::runtime_registration_bytes()?
            .checked_add(groups::group_book_registration_bytes()?)
            .and_then(|n| n.checked_add(groups::group_book_construction_scratch_bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)?
            .checked_add(groups::journal_registration_bytes()?)
            .and_then(|n| n.checked_add(groups::journal_construction_scratch_bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)?;
        let runtime_metadata = groups
            .checked_mul(
                std::mem::size_of::<Group>()
                    + std::mem::size_of::<groups::TransferRow>()
                    + std::mem::size_of::<groups::ArchivedOrigin>()
                    + groups::registration_bytes()?,
            )
            .and_then(|n| n.checked_add(shell_bytes))
            .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
            .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
            .and_then(|n| n.checked_add(claimed::reserved_state_bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)?;
        let extra = self
            .collection_bytes()?
            .checked_add(size_of::<
                Option<&mut crate::audio_transform::OriginalSamplerTransaction<'_>>,
            >())
            .ok_or(AudioStorageError::Overflow)?
            .checked_add(size_of::<OriginalSamplerConstruction>())
            .and_then(|n| n.checked_add(size_of::<storage::OriginalSamplerSealInspection>()))
            .ok_or(AudioStorageError::Overflow)?
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
        Ok(compiler)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mandatory actual constructor inputs"
    )]
    pub(crate) fn inspect_original_sampler_with_writers_on_control(
        &self,
        host: Option<&crate::sampler_host_fields::OriginalSamplerConstructionHost<'_, '_>>,
        target: &crate::SharedMetadata<hex_project::Project>,
        producer: &OriginalResourceBankPlan<'_>,
        resident: Option<&dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &crate::AudioRootStorageLedger<'_>,
        writers: &crate::audio_transform::OriginalSamplerTransaction<'_>,
        reservation: crate::AudioStorageLedgerReservation,
        purpose: ConstructorPurpose,
    ) -> Result<OriginalSamplerConstruction, AudioStorageError> {
        let compiler_reservation =
            self.original_sampler_compiler_reservation(target, reservation)?;
        let compiler = self
            .custody
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .inspect_original_sampler_revision_with_writers_on_control(
                host,
                target,
                resident,
                Some(producer),
                editor,
                previous,
                original,
                writers,
                compiler_reservation,
            )?;
        Ok(OriginalSamplerConstruction {
            compiler,
            compiler_reservation,
            cell_address: std::ptr::from_ref(self).addr(),
            target_address: std::ptr::from_ref(target).addr(),
            purpose,
            retire_after_native_guards: host.is_some(),
        })
    }

    pub(in crate::audio_candidate) fn construct_original_sampler_with_writers_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        writers: &mut crate::audio_transform::OriginalSamplerTransaction<'_>,
        construction: OriginalSamplerConstruction,
    ) -> Result<(), AudioStorageError> {
        // Only SAME-field/argument correlation. The private construction value
        // never crosses an external call, unlock, callback or adoption boundary.
        if construction.cell_address != std::ptr::from_ref(self).addr()
            || construction.target_address != std::ptr::from_ref(target).addr()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let custody = self
            .custody
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        self.token = Some(
            custody.construct_original_sampler_revision_with_writers_on_control(
                target,
                previous,
                writers,
                construction.compiler_reservation,
                construction.compiler,
                construction.retire_after_native_guards,
            )?,
        );
        self.finish_original_sampler_runtime_on_control(construction.purpose)
    }

    fn finish_original_sampler_runtime_on_control(
        &mut self,
        purpose: ConstructorPurpose,
    ) -> Result<(), AudioStorageError> {
        let custody = self
            .custody
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let draft = self
            .draft
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let token = self
            .token
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        match purpose {
            ConstructorPurpose::NativeFresh => draft.prepare_on_control(custody, token)?,
            ConstructorPurpose::MainExport => draft.prepare_main_on_control(custody, token)?,
        }
        draft.finish_on_control(&mut self.custody, &mut self.token, &mut self.owner)?;
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
