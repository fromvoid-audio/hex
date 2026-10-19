//! Same-parent complete compiled birth, still not native publication authority.
use super::*;
impl crate::AudioPreparationCustody {
    pub(super) fn sampler_completion_bytes(
        &self,
        project: &hex_project::Project,
    ) -> Result<usize, AudioStorageError> {
        if self.bank.is_some()
            || self.revision.capacity()?.reserved_handle_destinations != 0
            || self.structure.observe()?.bytes()? != 0
            || self.assembly.capacity_at(false, None)?.bytes()? != 0
            || self.tempo.observe(true)?.bytes()? != 0
            || self.shared.observe()?.reserved_handle_destinations != 0
            || !self.slots.is_empty()
            || project
                .session_cells()
                .iter()
                .any(|cell| !matches!(cell.content(), hex_project::SessionCellContent::Stop))
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let mut total = project
            .tempo_map()
            .owned_allocation_layout()
            .checked_backing_bytes()
            .ok_or(AudioStorageError::Overflow)?;
        for cost in [
            crate::session_structure_preparation::SessionStructureConstruction::compiler_construction_bytes(project)?,
            crate::session_inventory::audio_assembly::SessionAssembly::compiler_construction_bytes(project)?,
            crate::session_shared_preparation::SessionSharedConstruction::compiler_construction_bytes()?,
            crate::prepared_revision::assembly::RevisionAssembly::compiler_construction_bytes(project)?,
        ] {
            total = add(total, cost)?;
        }
        Ok(total)
    }
    /// Exact captured target, actual embedded bank and persistent constructor
    /// destination. No completed legacy Arc is adopted into a qualified reader.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "candidate bridge under construction")
    )]
    pub(crate) fn compile_original_sampler_revision_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        editor: &hex_project::Editor,
        previous: Option<&OwnedShared<SharedAnalyticContextTable>>,
        original: &AudioRootStorageLedger<'_>,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<crate::AudioPreparationToken, AudioStorageError> {
        self.inspect_sampler_destination()?;
        let next = self
            .incarnation
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        let construction = Inputs {
            rows: &self.arrangement_clips,
            preparation: self,
            editor,
            target: Some(target),
            resources: self
                .original_resources
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?,
            previous,
            original,
        }
        .inspect_with_completion(transforms, reservation, true)?;
        self.incarnation = next;
        self.phase = AudioPreparationPhase::Constructing;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.arrangement_clips.construct_original_sampler(
                target,
                self.original_resources
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?,
                previous,
                transforms,
                construction,
            )?;
            // The supported constructor has no ordinary audio sources. Its
            // real empty bank still receives the ordinary constructor identity;
            // no pending device PCM is copied into legacy Frame storage.
            crate::OwnedAudioInputs::try_new_in_place(
                &mut Vec::new(),
                crate::AudioStorageReservation {
                    // No ordinary audio sources: do not reserve an unrelated
                    // alias Vec from the global inspection row budget.
                    allocation_items: 0,
                    handle_obligations: 0,
                    max_prepared_data_bytes: reservation.max_retained_bytes,
                    max_construction_data_bytes: reservation.max_construction_peak_bytes,
                    caller_retained_bytes: 0,
                    caller_construction_bytes: 0,
                },
                &mut self.bank,
            )?;
            let bank = self
                .bank
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            self.session.reserve(target, next, bank, 0)?;
            self.structure.reserve(target, &self.session)?;
            self.tempo.reserve(target)?;
            self.tempo.construct()?;
            self.session.construct(bank)?;
            let contexts = self
                .arrangement_clips
                .root_context()
                .ok_or(AudioStorageError::SourceMismatch)?;
            self.shared.construct_compiler_session(contexts)?;
            self.assembly.reserve(
                target,
                &self.structure,
                &self.session,
                self.shared.inventory_handles(),
            )?;
            self.shared
                .validate_compiler_pair(contexts, self.assembly.shared_storage_at(None)?)?;
            self.assembly
                .construct_compiler_factories(target, contexts)?;
            self.assembly.construct(
                &mut self.session,
                &mut self.structure,
                &mut self.tempo,
                &mut self.shared,
                bank,
            )?;
            self.shared
                .validate_compiler_pair(contexts, self.assembly.shared_storage_at(None)?)?;
            let arrangement = self
                .arrangement_clips
                .root
                .project()
                .ok_or(AudioStorageError::SourceMismatch)?;
            self.revision.reserve_compiled(target, arrangement)?;
            self.revision
                .construct(&mut self.arrangement_clips, &mut self.assembly)?;
            Ok(())
        }));
        match outcome {
            Ok(Ok(())) => {
                self.phase = AudioPreparationPhase::ArrangementAndSessionSources;
                Ok(crate::AudioPreparationToken {
                    root: self.root,
                    incarnation: next,
                })
            }
            Ok(Err(error)) => {
                self.reason = Some(error);
                self.phase = AudioPreparationPhase::Cleaning;
                Err(error)
            }
            Err(payload) => {
                self.phase = AudioPreparationPhase::Cleaning;
                self.cleanup_on_control();
                self.phase = AudioPreparationPhase::FailedIncomplete;
                let secondary = self.first_panic.replace(payload);
                drop(secondary);
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}
