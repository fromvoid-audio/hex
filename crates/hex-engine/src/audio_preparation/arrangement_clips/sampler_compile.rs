//! Closed private control operation over actual borrowed inputs and persistent
//! destinations. Neither its arithmetic nor selected inventory admits playback.
use super::*;
mod complete;
use crate::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole,
    audio_transform::{AudioTransformPcmInventory, AudioTransformPreparation},
    native_device_resources::{DeviceResourceBank, DeviceResourcePreparation},
    owned_shared::OwnedShared,
    shared_analytic::{SamplerContextConstructionLimits, SharedAnalyticContextTable},
};

fn add(a: usize, b: usize) -> Result<usize, AudioStorageError> {
    a.checked_add(b).ok_or(AudioStorageError::Overflow)
}
#[derive(Clone, Copy)]
struct Construction {
    retained: usize,
    peak: usize,
    contexts: SamplerContextConstructionLimits,
    preview: bool,
}
impl ArrangementClips {
    #[cfg(test)]
    fn sampler_construction(
        &self,
        project: &hex_project::Project,
        resources: DeviceResourcePreparation<'_>,
    ) -> Result<Construction, AudioStorageError> {
        self.sampler_construction_with_preview(project, resources, false)
    }
    fn sampler_construction_with_preview(
        &self,
        project: &hex_project::Project,
        resources: DeviceResourcePreparation<'_>,
        preview: bool,
    ) -> Result<Construction, AudioStorageError> {
        if self.phase != Phase::Vacant
            || self.staged
            || self.pending_audio.is_some()
            || self.pending_stream.is_some()
            || self.pending_metadata.is_some()
            || self.source_disposal == Disposal::Pending
            || self.metadata_disposal == Disposal::Pending
            || !self.root.compiler_destinations_vacant()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.capacity_at(false, None, None)?;
        let tracks = self.tracks.compiler_selector_cost(project)?;
        let returns = self.returns.selector_construction_cost(project)?;
        let contexts =
            SharedAnalyticContextTable::sampler_construction_allowance(project, Some(resources))
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let compiler = crate::compile::sampler_cost::quote(project)
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let preview_cost = if preview {
            Some(crate::midi_preview::persistent::PreviewCustody::compiler_cost(project)?)
        } else {
            None
        };
        Ok(Construction {
            retained: add(
                add(
                    tracks.additional_retained_bytes,
                    returns.additional_retained_bytes,
                )?,
                add(
                    add(contexts.retained_bytes, compiler.retained)?,
                    preview_cost.map_or(0, |cost| cost.retained),
                )?,
            )?,
            peak: add(
                add(tracks.additional_peak_bytes, returns.additional_peak_bytes)?,
                add(
                    add(contexts.construction_peak_bytes, compiler.peak)?,
                    preview_cost.map_or(0, |cost| cost.peak),
                )?,
            )?,
            contexts,
            preview,
        })
    }
}
struct Inputs<'a> {
    rows: &'a ArrangementClips,
    preparation: &'a crate::AudioPreparationCustody,
    editor: &'a hex_project::Editor,
    target: Option<&'a crate::SharedMetadata<hex_project::Project>>,
    resources: &'a DeviceResourceBank,
    previous: Option<&'a OwnedShared<SharedAnalyticContextTable>>,
    original: &'a AudioRootStorageLedger<'a>,
}
impl<'a> Inputs<'a> {
    fn visit<'scope>(
        &self,
        inventory: &'scope AudioTransformPcmInventory<'scope>,
        ledger: &mut AudioRootStorageLedger<'scope>,
    ) -> Result<(), AudioStorageError>
    where
        'a: 'scope,
    {
        self.visit_at(inventory, ledger, true)
    }
    fn visit_at<'scope>(
        &self,
        inventory: &'scope AudioTransformPcmInventory<'scope>,
        ledger: &mut AudioRootStorageLedger<'scope>,
        standalone_parent: bool,
    ) -> Result<(), AudioStorageError>
    where
        'a: 'scope,
    {
        // Mandatory actual inputs FIRST. Importing old roots before these
        // visitors can turn legitimate repeated observations into root errors.
        if !matches!(
            self.preparation.phase(),
            AudioPreparationPhase::Vacant
                | AudioPreparationPhase::ArrangementSources
                | AudioPreparationPhase::ArrangementAndSessionSources
        ) {
            return Err(AudioStorageError::UnknownStorage);
        }
        // Closed-operation composition uses the actual typed visitor directly.
        // The public wrapper's outer mutate() invalidates its ledger until it
        // finishes, so nonempty nested visitors cannot reenter that wrapper.
        // ANY failure here aborts and discards this scoped ledger; no partial
        // ledger can escape or be resumed. Public wrapper behavior is unchanged.
        if standalone_parent {
            ledger.root(self.preparation, AudioStorageRole::Current)?;
        }
        self.preparation
            .visit_audio_storage(ledger, AudioStorageRole::Current)?;
        if !std::ptr::eq(self.rows, &self.preparation.arrangement_clips) {
            ledger.root(self.rows, AudioStorageRole::FailedConstruction)?;
            self.rows.visit_audio_storage(ledger, None, None)?;
        }
        ledger.root(self.editor, AudioStorageRole::Caller)?;
        ledger.borrow_editor_history_buffers_on_control(self.editor)?;
        ledger.borrow_editor_exclusive_buffers_on_control(self.editor)?;
        ledger.borrow_editor_midi_sources_on_control(self.editor)?;
        ledger.borrow_editor_native_states_on_control(self.editor)?;
        // A bank field already inside the mandatory preparation body has its
        // header charged there. A standalone bank needs its own actual root.
        if !self
            .preparation
            .original_resources
            .as_ref()
            .is_some_and(|bank| std::ptr::eq(bank, self.resources))
        {
            ledger.root(self.resources, AudioStorageRole::Caller)?;
        }
        self.resources.visit_storage_on_control(ledger)?;
        if let Some(target) = self.target {
            // This exact paired captured Project remains a distinct physical
            // input even after current authoring/history has discarded it.
            ledger.borrow_shared_metadata_body_on_control(target)?;
            ledger.borrow_project_exclusive_buffers_on_control(target)?;
            ledger.borrow_project_midi_sources_on_control(target)?;
            ledger.borrow_project_native_states_on_control(target)?;
        }
        if let Some(previous) = self.previous {
            crate::shared_analytic::resource_storage::visit_table(
                previous,
                ledger,
                AudioStorageRole::Predecessor,
            )?;
        }
        // Charge the actual enclosing preparation body, its cache/pending
        // backing and cancellation; do not confuse PCM-only visitation with it.
        inventory.visit_original_only_body_on_control(ledger)?;
        if std::ptr::eq(self.rows, &self.preparation.arrangement_clips) {
            ledger.require_original_inputs_bound(self.original)?;
        }
        ledger.original_ledger_storage(self.original)
    }
}
impl ArrangementClips {
    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "historical independent component oracle"
    )]
    pub(crate) fn compile_original_sampler_on_control(
        &mut self,
        editor: &hex_project::Editor,
        preparation: &crate::AudioPreparationCustody,
        bank: &DeviceResourceBank,
        previous: Option<&OwnedShared<SharedAnalyticContextTable>>,
        original: &AudioRootStorageLedger<'_>,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        let construction = Inputs {
            rows: self,
            preparation,
            editor,
            target: None,
            resources: bank,
            previous,
            original,
        }
        .inspect(transforms, reservation)?;
        self.construct_original_sampler(editor.project(), bank, previous, transforms, construction)
    }
    fn construct_original_sampler(
        &mut self,
        project: &hex_project::Project,
        bank: &DeviceResourceBank,
        previous: Option<&OwnedShared<SharedAnalyticContextTable>>,
        transforms: &AudioTransformPreparation,
        construction: Construction,
    ) -> Result<(), AudioStorageError> {
        let resources = DeviceResourcePreparation {
            bank,
            max_scope_metadata_bytes: 65_536,
        };
        // No arbitrary caller callback, exported quote or field mutation
        // intervenes before this same operation uses its persistent fields.
        if transforms.cancellation_requested_on_control() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let effects =
            hex_devices::native_registry().map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let result = transforms.compile_transaction(|| {
            self.root
                .begin_compiler_context()
                .map_err(|_| crate::Invalid("sampler context destination is occupied"))?;
            if construction.preview {
                self.preview
                    .reserve_compiler(project)
                    .map_err(|_| crate::Invalid("sampler preview reservation refused"))?;
            }
            let (destination, contexts) = self.root.compiler_destinations();
            *contexts = Some(
                SharedAnalyticContextTable::prepare_with_sampler_construction_limits(
                    project,
                    &instruments,
                    previous.map(|table| table.as_ref()),
                    Some(resources),
                    Some(construction.contexts),
                    None,
                )?,
            );
            if transforms.cancellation_requested_on_control() {
                return Err(crate::Invalid("audio preparation cancelled"));
            }
            self.returns
                .reserve(project)
                .map_err(|_| crate::Invalid("sampler paired Return preparation refused"))?;
            self.returns
                .construct()
                .map_err(|_| crate::Invalid("sampler paired Return construction refused"))?;
            let returns =
                super::super::CompilerReturnChildren::from_rows(&mut self.returns, project)
                    .map_err(|_| crate::Invalid("sampler paired Return selector refused"))?;
            let tracks = crate::audio_preparation::CompilerTrackChildren::prepare(
                &mut self.tracks,
                project,
                contexts.as_ref().map(|table| table.as_ref()),
            )
            .map_err(|_| crate::Invalid("sampler paired Track selector refused"))?;
            crate::compile::compile_project_into_selected_children(
                project,
                |_, _| {
                    Err(crate::Invalid(
                        "audio clips are outside sampler compiler class",
                    ))
                },
                if construction.preview {
                    Some(
                        self.preview
                            .compiler_destination()
                            .map_err(|_| crate::Invalid("sampler preview destination refused"))?,
                    )
                } else {
                    None
                },
                &effects,
                &instruments,
                contexts,
                transforms,
                destination,
                None,
                Some(returns),
                Some(tracks),
            )?;
            if construction.preview {
                self.preview
                    .finish_compiler()
                    .map_err(|_| crate::Invalid("sampler preview completion refused"))?;
            }
            Ok(())
        });
        #[cfg(test)]
        if let Err(error) = &result {
            eprintln!("closed sampler constructor error: {error:?}");
        }
        result.map_err(|_| AudioStorageError::UnsupportedGraph)
    }
}

impl Inputs<'_> {
    fn inspect(
        &self,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<Construction, AudioStorageError> {
        self.inspect_with_completion(transforms, reservation, false)
    }
    fn inspect_with_completion(
        &self,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
        complete: bool,
    ) -> Result<Construction, AudioStorageError> {
        if transforms.cancellation_requested_on_control() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let project = self
            .target
            .map_or(self.editor.project(), |target| &**target);
        let original = self.original;
        let resources = DeviceResourcePreparation {
            bank: self.resources,
            max_scope_metadata_bytes: 65_536,
        };
        let mut construction = self.rows.sampler_construction_with_preview(
            project,
            resources,
            self.target.is_some(),
        )?;
        if complete {
            let extra = self.preparation.sampler_completion_bytes(project)?;
            construction.retained = add(construction.retained, extra)?;
            construction.peak = add(construction.peak, extra)?;
        }
        let ledger_bytes = AudioRootStorageLedger::requested_metadata_bytes(reservation)?;
        let arithmetic_inline = size_of::<AudioRootStorageLedger<'_>>();
        // The original ledger's physical body/backings are DATA via Inputs;
        // its still-required live growth is a SEPARATE peak allowance.
        let original_growth = original.original_live_growth_allowance()?;
        transforms.with_pcm_inventory_on_control(|inventory| {
            let cache = inventory.original_only_storage_cost()?;
            let retained_extra = add(
                add(construction.retained, ledger_bytes)?,
                add(arithmetic_inline, cache.transaction_new_backing_bytes)?,
            )?;
            let peak_extra = add(
                add(construction.peak, ledger_bytes)?,
                add(
                    add(arithmetic_inline, cache.transaction_new_backing_bytes)?,
                    original_growth,
                )?,
            )?;
            let retained_data = reservation
                .max_retained_bytes
                .checked_sub(retained_extra)
                .ok_or(AudioStorageError::ReservationExceeded)?;
            let peak_data = reservation
                .max_construction_peak_bytes
                .checked_sub(peak_extra)
                .ok_or(AudioStorageError::ReservationExceeded)?;
            AudioRootStorageLedger::arithmetic_cost_for_limits(
                reservation,
                retained_data,
                peak_data,
                |ledger| self.visit(inventory, ledger),
            )?;
            // Allocate only AFTER both bound checks. This normal pass MUST NOT
            // be omitted even when cheap arithmetic fitted without replay.
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            self.visit(inventory, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let actual = ledger.summary()?;
            if ledger.metadata_capacity_bytes() != Some(ledger_bytes)
                || add(actual.data_bytes, retained_extra)? > reservation.max_retained_bytes
                || add(
                    add(
                        actual.data_bytes,
                        actual
                            .construction_peak_bytes
                            .checked_sub(actual.retained_bytes)
                            .ok_or(AudioStorageError::Overflow)?,
                    )?,
                    peak_extra,
                )? > reservation.max_construction_peak_bytes
            {
                return Err(AudioStorageError::ReservationExceeded);
            }
            // Scoped ledger and borrowed records die WITHIN the three guards.
            Ok(())
        })?;
        Ok(construction)
    }
}
impl crate::AudioPreparationCustody {
    pub(crate) fn visit_sampler_candidate_inputs<'scope>(
        &'scope self,
        target: &'scope crate::SharedMetadata<hex_project::Project>,
        editor: &'scope hex_project::Editor,
        previous: Option<&'scope OwnedShared<SharedAnalyticContextTable>>,
        original: &'scope AudioRootStorageLedger<'scope>,
        inventory: &'scope AudioTransformPcmInventory<'scope>,
        ledger: &mut AudioRootStorageLedger<'scope>,
    ) -> Result<(), AudioStorageError> {
        let inputs = Inputs {
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
        };
        // Caller is the actual CandidateOwner closed operation: its existing
        // CandidateState Box contains this inline parent and charges that body.
        // This changes only enclosing-field accounting, not mandatory children.
        inputs.visit_at(inventory, ledger, false)
    }
    /// Construction-local only. Every new owner is installed directly into
    /// this SAME parent; immutable whole-parent inventory ends before mutation.
    #[cfg(test)]
    pub(crate) fn compile_original_sampler_on_control(
        &mut self,
        editor: &hex_project::Editor,
        bank: &DeviceResourceBank,
        previous: Option<&OwnedShared<SharedAnalyticContextTable>>,
        original: &AudioRootStorageLedger<'_>,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        self.inspect_sampler_destination()?;
        let construction = Inputs {
            rows: &self.arrangement_clips,
            preparation: self,
            editor,
            target: None,
            resources: bank,
            previous,
            original,
        }
        .inspect(transforms, reservation)?;
        // No exported approval or callback intervenes. Safe disjoint borrowing
        // retains the parent and all of its source/cleanup destinations in place.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.arrangement_clips.construct_original_sampler(
                editor.project(),
                bank,
                previous,
                transforms,
                construction,
            )
        })) {
            Ok(result) => result,
            Err(payload) => {
                // The constructor unwound with owners still in THIS parent.
                // Retire once through its existing ordered cleanup. The first
                // payload stays owned locally during cleanup, then is installed
                // before any secondary cleanup payload can be dropped.
                self.phase = AudioPreparationPhase::Cleaning;
                self.cleanup_on_control();
                self.phase = AudioPreparationPhase::FailedIncomplete;
                let secondary = self.first_panic.replace(payload);
                drop(secondary);
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    /// Exact immutable captured target, with current authoring/history retained
    /// only as additional inventory. The original bank is borrowed INSIDE this
    /// actual parent operation and cannot be substituted/extracted by a caller.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "complete target bridge in progress")
    )]
    pub(crate) fn compile_original_sampler_target_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        editor: &hex_project::Editor,
        previous: Option<&OwnedShared<SharedAnalyticContextTable>>,
        original: &AudioRootStorageLedger<'_>,
        transforms: &AudioTransformPreparation,
        reservation: AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        self.inspect_sampler_destination()?;
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
        .inspect(transforms, reservation)?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let bank = self
                .original_resources
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            self.arrangement_clips.construct_original_sampler(
                target,
                bank,
                previous,
                transforms,
                construction,
            )
        }));
        match outcome {
            Ok(result) => result,
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
    fn inspect_sampler_destination(&self) -> Result<(), AudioStorageError> {
        if !matches!(
            self.phase,
            AudioPreparationPhase::Vacant
                | AudioPreparationPhase::ArrangementSources
                | AudioPreparationPhase::ArrangementAndSessionSources
        ) || self.first_panic.is_some()
            || self.original_resources_drop_pending
            || self
                .slots
                .iter()
                .any(|slot| slot.disposal == Disposal::Pending)
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        // Actual restoration destinations and pending owners are validated by
        // the mandatory whole-parent visitor, not by Vacant or scalar summary.
        self.arrangement_clips.root.preflight_restore(
            &self.arrangement_clips.tracks,
            &self.arrangement_clips.returns,
            &self.shared,
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests;
