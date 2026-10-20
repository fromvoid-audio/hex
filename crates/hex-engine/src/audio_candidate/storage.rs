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
// Private Engine continuation only. The actual constructor metadata stays
// within this closed operation; it is never an admission/retirement permit.
// Installing metadata requires a NEW immutable inspection before final sealing.
pub(crate) struct OriginalSamplerSealInspection {
    summary: AudioCandidateConstructorSummary,
    physical: Option<crate::audio_storage::NativePhysicalCapture>,
    owner_address: usize,
    identity: AudioCandidateIdentity,
}
impl AudioCandidateOwner {
    pub(super) fn require_unpublished_native_host_class(&self) -> Result<(), AudioStorageError> {
        let inner = self.inner.as_ref();
        if inner.phase != AudioCandidateDisposalPhase::Owned
            || inner.runtime_receipt.is_some()
            || inner.first_panic.is_some()
            || inner.receipt_taken
            || inner.recovered_inputs.is_some()
            || inner.inputs_drop_pending
            || inner.inputs_disposed
            || inner
                .runtime
                .as_ref()
                .is_none_or(|runtime| runtime.playback.is_main_control_runtime())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        inner.validate_groups()?;
        inner.validate_fresh_origins()?;
        Ok(())
    }
    pub(super) fn visit_unpublished_native_host_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.require_unpublished_native_host_class()?;
        let inner = self.inner.as_ref();
        inner
            .custody
            .visit_audio_storage(ledger, crate::AudioStorageRole::Current)?;
        inner.visit_fresh_storage(ledger)
    }

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
        if self
            .inner
            .runtime
            .as_ref()
            .is_some_and(|r| r.playback.is_main_control_runtime())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
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
        &mut self,
        reservation: AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        // This closed operation owns the local ledger through final closure.
        // Child typed visitors may use mutate themselves; an outer mutation
        // wrapper would make their actual nonempty map observations reentrant.
        // Any failure discards this entire local ledger, never a resumed prefix.
        ledger.root(self, crate::AudioStorageRole::Current)?;
        self.inner
            .custody
            .visit_audio_storage(&mut ledger, crate::AudioStorageRole::Current)?;
        let before = ledger.summary()?.data_bytes;
        self.inner.visit_fresh_storage(&mut ledger)?;
        let summary = self.finish_constructor_inspection(
            &ledger,
            before,
            reservation,
            external_runtime_bytes,
        )?;
        if !self.inner.archive.has_physical() {
            let physical = ledger.capture_native_physical_on_control(
                summary.reserved_split_bytes,
                crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                    .checked_sub(summary.owned_runtime_bytes)
                    .ok_or(AudioStorageError::ReservationExceeded)?,
                &self.inner.custody,
                &self.inner.token,
                &self.inner.archive,
            )?;
            // All borrowed actual rows end before installing the new immutable
            // metadata into the SAME exclusive constructor. Nothing escapes.
            drop(ledger);
            self.inner.archive.install_physical(physical)?;
            return self.inspect_constructor_storage(reservation, external_runtime_bytes);
        }
        Ok(summary)
    }
    fn finish_constructor_inspection(
        &self,
        ledger: &AudioRootStorageLedger<'_>,
        before: usize,
        reservation: AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        let data = ledger
            .summary()?
            .require_known_storage()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        ledger.validate_closed_shared_handles()?;
        let owned_runtime_bytes = data
            .data_bytes
            .checked_sub(before)
            .ok_or(AudioStorageError::Overflow)?;
        let future_wrapper = !self.inner.archive.wrapper_is_live();
        let reserved_split_bytes = if future_wrapper {
            ledger.require_future_claimed_wrapper_rows()?;
            claimed::reserved_state_bytes()?
        } else {
            // Actual Box+record are ALREADY in normal DATA. Only the actual
            // exclusively-borrowed prepared value remains as stack overlap.
            size_of::<AudioCandidatePreparedSplit<'_>>()
        };
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
        let (mut shared_resource_groups, mut shared_resource_handles) =
            ledger.shared_resource_counts();
        if future_wrapper {
            // Capacity for the finite new metadata Arc and its two owning
            // fields before birth; never declarations or identity authority.
            shared_resource_groups = shared_resource_groups
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
            shared_resource_handles = shared_resource_handles
                .checked_add(2)
                .ok_or(AudioStorageError::Overflow)?;
        }
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
    /// Engine-private fixed actual-input traversal, consumed by the same closed
    /// preparation operation that constructed this target. This grants no
    /// Service/native source association or whole-host completion authority.
    #[expect(
        clippy::too_many_arguments,
        reason = "closed actual captured and producer input operation"
    )]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "native source host consumer in progress")
    )]
    pub(crate) fn seal_sampler_constructor_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        producer: Option<&crate::native_device_resources::OriginalResourceBankPlan<'_>>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &AudioRootStorageLedger<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        self.seal_sampler_constructor_for_purpose_on_control(
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
        reason = "closed actual input and purpose operation"
    )]
    pub(super) fn seal_sampler_constructor_for_purpose_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        producer: Option<&crate::native_device_resources::OriginalResourceBankPlan<'_>>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &AudioRootStorageLedger<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
        purpose: ConstructorPurpose,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        transforms.with_pcm_inventory_on_control(|inventory| {
            self.seal_sampler_constructor_with_inventory_on_control(
                target,
                None,
                producer,
                editor,
                previous,
                original,
                inventory,
                reservation,
                purpose,
            )
        })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "closed actual input and purpose operation"
    )]
    pub(super) fn seal_sampler_constructor_with_writers_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        resident: Option<&dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
        producer: Option<&crate::native_device_resources::OriginalResourceBankPlan<'_>>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &AudioRootStorageLedger<'_>,
        writers: &crate::audio_transform::OriginalSamplerTransaction<'_>,
        reservation: AudioStorageLedgerReservation,
        purpose: ConstructorPurpose,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        let inventory = writers.inventory();
        self.seal_sampler_constructor_with_inventory_on_control(
            target,
            resident,
            producer,
            editor,
            previous,
            original,
            &inventory,
            reservation,
            purpose,
        )
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "closed actual input and purpose operation"
    )]
    pub(super) fn seal_sampler_constructor_with_inventory_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        resident: Option<&dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
        producer: Option<&crate::native_device_resources::OriginalResourceBankPlan<'_>>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &AudioRootStorageLedger<'_>,
        inventory: &crate::audio_transform::AudioTransformPcmInventory<'_>,
        reservation: AudioStorageLedgerReservation,
        purpose: ConstructorPurpose,
    ) -> Result<AudioCandidateConstructorSummary, AudioStorageError> {
        self.inner.constructor_data = None;
        let inspection = self.inspect_sampler_constructor_with_inventory_on_control(
            None,
            target,
            resident,
            producer,
            editor,
            previous,
            original,
            inventory,
            reservation,
            purpose,
        )?;
        // All inspection records/ledgers have ended before this mutable loan.
        if let Some(summary) = self.install_sampler_constructor_inspection_on_control(inspection)? {
            return Ok(summary);
        }
        self.seal_sampler_constructor_with_inventory_on_control(
            target,
            resident,
            producer,
            editor,
            previous,
            original,
            inventory,
            reservation,
            purpose,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "closed actual input and purpose operation"
    )]
    pub(super) fn inspect_sampler_constructor_with_inventory_on_control(
        &self,
        host: Option<&crate::sampler_host_fields::OriginalSamplerConstructionHost<'_, '_>>,
        target: &crate::SharedMetadata<hex_project::Project>,
        resident: Option<&dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
        producer: Option<&crate::native_device_resources::OriginalResourceBankPlan<'_>>,
        editor: &hex_project::Editor,
        previous: Option<
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        >,
        original: &AudioRootStorageLedger<'_>,
        inventory: &crate::audio_transform::AudioTransformPcmInventory<'_>,
        reservation: AudioStorageLedgerReservation,
        purpose: ConstructorPurpose,
    ) -> Result<OriginalSamplerSealInspection, AudioStorageError> {
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
        let (summary, physical) = {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.root(self, crate::AudioStorageRole::Current)?;
            self.inner.custody.visit_sampler_candidate_inputs(
                host,
                target,
                resident,
                producer,
                editor,
                previous,
                original,
                inventory,
                &mut ledger,
            )?;
            let before = ledger.summary()?.data_bytes;
            self.inner.visit_constructor_storage(&mut ledger, purpose)?;
            let summary = self.finish_constructor_inspection(&ledger, before, reservation, 0)?;
            let physical = if purpose == ConstructorPurpose::NativeFresh
                && !self.inner.archive.has_physical()
            {
                Some(
                    ledger.capture_native_physical_on_control(
                        summary.reserved_split_bytes,
                        crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                            .checked_sub(summary.owned_runtime_bytes)
                            .ok_or(AudioStorageError::ReservationExceeded)?,
                        &self.inner.custody,
                        &self.inner.token,
                        &self.inner.archive,
                    )?,
                )
            } else {
                None
            };
            (summary, physical)
        };
        Ok(OriginalSamplerSealInspection {
            summary,
            physical,
            owner_address: std::ptr::from_ref(self).addr(),
            identity: self.identity(),
        })
    }

    pub(super) fn install_sampler_constructor_inspection_on_control(
        &mut self,
        inspection: OriginalSamplerSealInspection,
    ) -> Result<Option<AudioCandidateConstructorSummary>, AudioStorageError> {
        if inspection.owner_address != std::ptr::from_ref(self).addr()
            || inspection.identity != self.identity()
            || self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.first_panic.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(physical) = inspection.physical {
            self.inner.archive.install_physical(physical)?;
            // The new actual owning fields are NOT implicitly closed by the
            // inspection that preceded their birth. Reinspect all real fields.
            return Ok(None);
        }
        self.inner.constructor_data = Some(inspection.summary);
        Ok(Some(inspection.summary))
    }
}
impl CandidateState {
    pub(super) fn validate_fresh_origins(&self) -> Result<(), AudioStorageError> {
        self.archive.validate_fresh_pairs(&self.groups)?;
        self.archive.validate_journal_pair(&self.journal)?;
        self.archive.validate_book_pair(&self.groups)?;
        self.archive.validate_runtime_pair(
            self.runtime_lease
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?,
        )?;
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
        self.visit_constructor_storage(l, ConstructorPurpose::NativeFresh)
    }
    fn visit_constructor_storage<'a>(
        &'a self,
        l: &mut AudioRootStorageLedger<'a>,
        purpose: ConstructorPurpose,
    ) -> Result<(), AudioStorageError> {
        let Self {
            identity: _,
            custody: _,
            token: _,
            runtime,
            runtime_lease,
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
        archive.visit_registrations(l)?;
        groups.visit(l)?;
        journal.visit(l)?;
        l.vec(&archive.groups)?;
        let runtime = runtime.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
        let RuntimeOwner {
            playback,
            #[cfg(any(test, feature = "instrument-test-support"))]
            probe,
        } = runtime.as_ref();
        #[cfg(any(test, feature = "instrument-test-support"))]
        if probe.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        l.with_native_constructor_partition(
            crate::audio_storage::NativeRuntimePartition::Shell,
            |l| {
                runtime_lease
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?
                    .visit(l)?;
                for group in groups {
                    if let Some(lease) = &group.lease {
                        l.with_native_constructor_partition(
                            crate::audio_storage::NativeRuntimePartition::Group(
                                group.location.0,
                                group.location.1,
                            ),
                            |l| lease.visit(l),
                        )?;
                    }
                }
                l.bytes(
                    Owner::exclusive(runtime),
                    Location("runtime-owner-box", 0),
                    std::mem::size_of::<RuntimeOwner>(),
                )?;
                match purpose {
                    ConstructorPurpose::NativeFresh => playback.visit_fresh_candidate_storage(l),
                    ConstructorPurpose::MainExport => playback.visit_main_control_storage(l, true),
                }
            },
        )
    }
}

impl AudioCandidateAnchors {
    pub(crate) fn visit_transferred_graph_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.validate_transferred_runtime_fields()?;
        self.owner
            .inner
            .custody
            .visit_audio_storage(ledger, crate::AudioStorageRole::Archive)?;
        self.visit_transferred_runtime_fields(ledger)
    }

    /// One closed composition: the actual Sampler input walker includes this
    /// exact enclosing custody. Do not repeat fixed constructor handle rows by
    /// invoking the standalone whole-anchor visitor after that same walk.
    #[expect(
        clippy::too_many_arguments,
        reason = "same closed mandatory Sampler inputs"
    )]
    #[cfg(test)]
    pub(in crate::audio_candidate) fn visit_transferred_sampler_inputs<'a>(
        &'a self,
        target: &'a crate::SharedMetadata<hex_project::Project>,
        resident: Option<&'a dyn crate::sampler_host_fields::OriginalSamplerResidentFields>,
        producer: Option<&'a crate::native_device_resources::OriginalResourceBankPlan<'a>>,
        editor: &'a hex_project::Editor,
        previous: Option<
            &'a crate::owned_shared::OwnedShared<
                crate::shared_analytic::SharedAnalyticContextTable,
            >,
        >,
        original: &'a AudioRootStorageLedger<'a>,
        inventory: &'a crate::audio_transform::AudioTransformPcmInventory<'a>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.validate_transferred_runtime_fields()?;
        self.owner.inner.custody.visit_sampler_candidate_inputs(
            None, target, resident, producer, editor, previous, original, inventory, ledger,
        )?;
        self.visit_transferred_runtime_fields(ledger)
    }

    fn validate_transferred_runtime_fields(&self) -> Result<(), AudioStorageError> {
        let inner = self.owner.inner.as_ref();
        if !matches!(
            inner.phase,
            AudioCandidateDisposalPhase::Leased | AudioCandidateDisposalPhase::RootsCleaning
        ) || inner.runtime.is_some()
            || inner.runtime_lease.is_some()
            || inner.first_panic.is_some()
            || inner.receipt_taken
            || inner.constructor_data.is_none()
            || !inner.archive.has_wrapper_registration()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }

    fn visit_transferred_runtime_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.validate_transferred_runtime_fields()?;
        let inner = self.owner.inner.as_ref();
        ledger.bytes(
            Owner::exclusive(inner),
            Location("candidate-state-box", 0),
            size_of::<CandidateState>(),
        )?;
        inner.groups.visit(ledger)?;
        inner.journal.visit(ledger)?;
        ledger.vec(&inner.archive.groups)?;
        inner.archive.visit_registrations(ledger)?;
        inner.archive.visit_transferred_graph(ledger)
    }
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual transferred DSP graph plus its retained constructor source and
    /// birth-archive metadata. Native State/wrapper, channel/carrier/domain and
    /// transport backing must ALSO be visited by the host's finite schema.
    /// This grants neither whole-host admission nor publication readiness.
    pub fn borrow_candidate_transferred_graph_on_control(
        &mut self,
        anchors: &'a AudioCandidateAnchors,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| anchors.visit_transferred_graph_fields(ledger))
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
    fn constructor_capture_keeps_new_origins_and_charges_old_graph_in_whole_union() {
        fn inspect<'a>(
            new: &'a AudioCandidateOwner,
            old: Option<&'a AudioCandidateAnchors>,
            limits: AudioStorageLedgerReservation,
        ) -> AudioRootStorageLedger<'a> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
            if let Some(old) = old {
                ledger.root(old, crate::AudioStorageRole::Archive).unwrap();
                old.visit_transferred_graph_fields(&mut ledger).unwrap();
            }
            ledger.root(new, crate::AudioStorageRole::Current).unwrap();
            new.inner
                .custody
                .visit_audio_storage(&mut ledger, crate::AudioStorageRole::Current)
                .unwrap();
            new.inner.visit_fresh_storage(&mut ledger).unwrap();
            ledger.validate_closed_shared_handles().unwrap();
            ledger
        }
        fn footprint(
            physical: &crate::audio_storage::NativePhysicalCapture,
            new: &AudioCandidateOwner,
        ) -> (usize, usize) {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
            physical
                .visit_exclusive_partition(
                    crate::audio_storage::NativeRuntimePartition::Shell,
                    &mut ledger,
                )
                .unwrap();
            for group in &new.inner.groups {
                physical
                    .visit_exclusive_partition(
                        crate::audio_storage::NativeRuntimePartition::Group(
                            group.location.0,
                            group.location.1,
                        ),
                        &mut ledger,
                    )
                    .unwrap();
            }
            let summary = ledger.summary().unwrap();
            (summary.data_allocations, summary.data_bytes)
        }
        let (old_custody, old_token, _, _) =
            crate::audio_preparation::candidate_test_fixture(false).unwrap();
        let mut old = AudioCandidateOwner::try_new_on_control(old_custody, old_token)
            .ok()
            .unwrap();
        old.seal_constructor_on_control(reservation()).unwrap();
        let (mut old_runtime, old_anchor) = old.split_constructor_on_control().ok().unwrap();
        let (new_custody, new_token, _, _) =
            crate::audio_preparation::candidate_test_fixture(false).unwrap();
        let mut new = AudioCandidateOwner::try_new_on_control(new_custody, new_token)
            .ok()
            .unwrap();
        let split = claimed::reserved_state_bytes().unwrap();
        let (new_footprint, new_data) = {
            let ledger = inspect(&new, None, reservation());
            let physical = ledger
                .capture_native_physical_on_control(
                    split,
                    crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                    &new.inner.custody,
                    &new.inner.token,
                    &new.inner.archive,
                )
                .unwrap();
            let found = footprint(&physical, &new);
            assert!(found.0 > 0 && found.1 > 0);
            (found, ledger.summary().unwrap().data_bytes)
        };
        let (summary, backing) = {
            let whole = inspect(&new, Some(&old_anchor), reservation());
            let summary = whole.summary().unwrap();
            assert!(summary.data_bytes > new_data);
            let mut result = None;
            let measured = allocation_counter::measure(|| {
                result = Some(whole.capture_native_physical_on_control(
                    split,
                    crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                    &new.inner.custody,
                    &new.inner.token,
                    &new.inner.archive,
                ))
            });
            let physical = result.unwrap().unwrap();
            assert_eq!(footprint(&physical, &new), new_footprint);
            assert_eq!(
                (measured.count_total, measured.bytes_total),
                (
                    u64::try_from(measured.count_current).unwrap(),
                    u64::try_from(measured.bytes_current).unwrap()
                )
            );
            (summary, usize::try_from(measured.bytes_total).unwrap())
        };
        // Real whole DATA + BOTH live ledger/body/scratch costs remain in the
        // bound. Nothing imports, removes or reinterprets old partition tags.
        for (retained_minus, peak_minus) in [(1, 0), (0, 1), (0, 0)] {
            let mut limits = reservation();
            limits.max_retained_bytes = summary.retained_bytes + split + backing - retained_minus;
            limits.max_construction_peak_bytes = summary.construction_peak_bytes
                + split
                + backing
                + size_of::<crate::audio_storage::NativePhysicalCapture>()
                - peak_minus;
            let whole = inspect(&new, Some(&old_anchor), limits);
            let mut result = None;
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            let measured = allocation_counter::measure(|| {
                result = Some(whole.capture_native_physical_on_control(
                    split,
                    crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                    &new.inner.custody,
                    &new.inner.token,
                    &new.inner.archive,
                ))
            });
            if retained_minus != 0 || peak_minus != 0 {
                assert!(matches!(
                    result,
                    Some(Err(AudioStorageError::ReservationExceeded))
                ));
                assert_eq!((measured.count_total, measured.count_current), (0, 0));
                assert_eq!(
                    (
                        crate::owned_shared::birth_count(),
                        crate::owned_shared::clone_count()
                    ),
                    (0, 0)
                );
            } else {
                let physical = result.unwrap().unwrap();
                assert_eq!(footprint(&physical, &new), new_footprint);
            }
            assert_eq!(
                old_anchor.owner.phase(),
                AudioCandidateDisposalPhase::Leased
            );
        }
        new.dispose_on_control().unwrap();
        let proof = old_runtime.dispose_on_control().unwrap();
        let mut anchors = [Some(old_anchor)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        drop(proof);
        drop(old_runtime);
        anchors[0]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
    }
    #[test]
    fn retained_physical_metadata_caps_refuse_before_backing_or_reader_birth() {
        let Some((custody, token, _, _)) = crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        fn inspect<'a>(
            owner: &'a AudioCandidateOwner,
            reservation: AudioStorageLedgerReservation,
        ) -> AudioRootStorageLedger<'a> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
            ledger
                .root(owner, crate::AudioStorageRole::Current)
                .unwrap();
            owner
                .inner
                .custody
                .visit_audio_storage(&mut ledger, crate::AudioStorageRole::Current)
                .unwrap();
            owner.inner.visit_fresh_storage(&mut ledger).unwrap();
            ledger.validate_closed_shared_handles().unwrap();
            ledger
        }
        let ledger = inspect(&owner, reservation());
        let base = ledger.summary().unwrap();
        let split = claimed::reserved_state_bytes().unwrap();
        let mut physical = None;
        let measured = allocation_counter::measure(|| {
            physical = Some(
                ledger
                    .capture_native_physical_on_control(
                        split,
                        crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                        &owner.inner.custody,
                        &owner.inner.token,
                        &owner.inner.archive,
                    )
                    .unwrap(),
            );
        });
        // Independent allocator observation, rather than repeating the quote's
        // element-size arithmetic. Registration clones its paired owners only.
        assert_eq!(measured.count_total, 2);
        assert_eq!(measured.count_current, 2);
        let backing = usize::try_from(measured.bytes_total).unwrap();
        assert!(backing > 0);
        drop(physical.take());
        drop(ledger);
        let exact_retained = base.retained_bytes + split + backing;
        let exact_peak = base.construction_peak_bytes
            + split
            + backing
            + size_of::<crate::audio_storage::NativePhysicalCapture>();
        for (retained_minus, peak_minus) in [(1, 0), (0, 1), (0, 0)] {
            let mut limits = reservation();
            limits.max_retained_bytes = exact_retained - retained_minus;
            limits.max_construction_peak_bytes = exact_peak - peak_minus;
            let ledger = inspect(&owner, limits);
            let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get());
            let births = crate::owned_shared::birth_count();
            let clones = crate::owned_shared::clone_count();
            let mut result = None;
            let allocation = allocation_counter::measure(|| {
                result = Some(ledger.capture_native_physical_on_control(
                    split,
                    crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
                    &owner.inner.custody,
                    &owner.inner.token,
                    &owner.inner.archive,
                ));
            });
            if retained_minus != 0 || peak_minus != 0 {
                assert_eq!(
                    result.as_ref().unwrap().as_ref().err(),
                    Some(&AudioStorageError::ReservationExceeded)
                );
                assert_eq!(
                    (
                        allocation.count_total,
                        allocation.count_current,
                        allocation.bytes_total,
                        allocation.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                assert_eq!(
                    crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get()),
                    ids
                );
                assert_eq!(crate::owned_shared::birth_count(), births);
                assert_eq!(crate::owned_shared::clone_count(), clones);
            } else {
                assert!(result.as_ref().unwrap().is_ok());
                assert_eq!(allocation.count_total, 2);
                assert_eq!(usize::try_from(allocation.bytes_total).unwrap(), backing);
                assert_eq!(
                    crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get()),
                    ids + 2
                );
            }
            drop(result);
            drop(ledger);
        }
        assert!(!owner.inner.archive.has_physical());
        owner.dispose_on_control().unwrap();
        owner.recover_inputs_on_control().unwrap();
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
