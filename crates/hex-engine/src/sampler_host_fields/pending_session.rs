//! ONE audited original-Sampler Session producer, not a mutable callback,
//! general transaction, transferable quote or downstream adoption capability.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};

pub trait OriginalSamplerPendingSession<'owner> {
    fn pending_context_on_control(&self) -> Result<crate::AudioSourceContext, AudioStorageError>;
    type Fields<'scope>: OriginalSamplerResidentFields
    where
        Self: 'scope;
    fn fields_and_writer_on_control<'scope>(
        &'scope mut self,
    ) -> (
        Self::Fields<'scope>,
        &'scope mut crate::audio_transform::OriginalSamplerTransaction<'owner>,
    );
    fn clone_source_on_control(&self) -> Result<Option<&hex_project::Project>, AudioStorageError>;
    fn replacement_elements_on_control(&self) -> Result<(usize, usize), AudioStorageError>;
    fn command_on_control(&self) -> Result<Option<&hex_project::Command>, AudioStorageError>;
    /// The sole Engine caller runs full actual union bootstrap+normal binding
    /// and BOTH caps before this mutation. Method visibility is not authority;
    /// source policy restricts caller/implementation to the closed driver and
    /// actual Session owner, preserving the selected source/guard lifetimes.
    fn stage_and_capture_on_control(&mut self) -> Result<bool, AudioStorageError>;
    fn source_and_writer_on_control<'scope>(
        &'scope mut self,
    ) -> Result<
        (
            Self::Fields<'scope>,
            CapturedSourceFields<'scope>,
            &'scope mut crate::audio_transform::OriginalSamplerTransaction<'owner>,
        ),
        AudioStorageError,
    >;
}

fn add(a: usize, b: usize) -> Result<usize, AudioStorageError> {
    a.checked_add(b).ok_or(AudioStorageError::Overflow)
}
struct PrefixCost {
    retained: usize,
    peak: usize,
}
fn metadata_cost<'owner, P: OriginalSamplerPendingSession<'owner>>(
    session: &P,
    source: &CapturedSourceFields<'_>,
    reservation: AudioStorageLedgerReservation,
) -> Result<PrefixCost, AudioStorageError> {
    let Some(target) = session.clone_source_on_control()? else {
        return Ok(PrefixCost {
            retained: 0,
            peak: 0,
        });
    };
    // These exact commands mutate scalars only. Dynamic owning commands need
    // their own concrete apply/growth helpers before this class can stage them.
    if let Some(command) = session.command_on_control()? {
        use hex_project::Command;
        match command {
            Command::Gain { track: _, gain: _ }
            | Command::MasterGain { gain: _ }
            | Command::SetLoopRegion {
                start: _,
                end: _,
                enabled: _,
            }
            | Command::ClearLoopRegion
            | Command::ToggleLoopRegion => {}
            _ => return Err(AudioStorageError::UnknownStorage),
        }
    }
    let buffers = target.storage_buffers_on_control();
    // Nonempty authored-routing validation allocates BTreeSet scratch. This
    // original class does not guess its allocator/layout bound.
    if !buffers.audio_feeds().is_empty()
        || !buffers.midi_routes().is_empty()
        || target.import_provenance().is_some()
        || target
            .tracks()
            .iter()
            .any(|t| t.clips().iter().any(|c| c.audio_edit().is_some()))
    {
        return Err(AudioStorageError::UnknownStorage);
    }
    let cloned = AudioRootStorageLedger::arithmetic_cost_for_limits(
        reservation,
        usize::MAX,
        usize::MAX,
        |ledger| ledger.mutate(|ledger| ledger.project_exclusive_buffers(target)),
    )?
    .data_bytes;
    let (undo, redo) = session.replacement_elements_on_control()?;
    let history = add(undo, redo)?
        .checked_mul(size_of::<hex_project::Project>())
        .ok_or(AudioStorageError::Overflow)?;
    let bindings = source
        .payload
        .bindings()
        .requested_backing_for_capacity(target.asset_entries_on_control().len())?;
    // Both new Project-exclusive copies coexist: Editor candidate and immutable
    // captured SharedProject. Shared MIDI/state DATA is not cloned and remains
    // the mandatory physical union; its individual new fields join after stage.
    let retained = add(add(cloned, cloned)?, add(history, bindings)?)?;
    let retained = add(
        retained,
        add(
            SharedMetadataRef::constructor_bytes::<hex_project::Project>()?,
            source.owner.same_payload_constructor_bytes()?,
        )?,
    )?;
    // Stack payloads passed to the two existing sized Arc constructors coexist
    // with their destination allocation. No opaque allocator/RSS claim follows.
    let peak = add(
        retained,
        add(
            size_of::<hex_project::Project>(),
            source.payload.body().layout_bytes(),
        )?,
    )?;
    Ok(PrefixCost { retained, peak })
}

fn preflight_prefix<'a>(
    host: &'a super::original_driver::OriginalSamplerConstructionHost<'a, '_>,
    preparation: &'a crate::AudioPreparationCustody,
    current_source: &'a CapturedSourceFields<'a>,
    resident: &'a dyn OriginalSamplerResidentFields,
    inventory: &'a crate::audio_transform::AudioTransformPcmInventory<'a>,
    cost: &PrefixCost,
    reservation: AudioStorageLedgerReservation,
) -> Result<(), AudioStorageError> {
    let visit = |ledger: &mut AudioRootStorageLedger<'a>| {
        host.worker.visit_current_constructor_owner(ledger)?;
        host.visit(preparation, Some(current_source.payload.project()), ledger)?;
        super::visit_original_sampler_resident(resident, inventory, ledger)
    };
    let backing = AudioRootStorageLedger::requested_metadata_bytes(reservation)?;
    let scratch = add(
        add(
            size_of::<AudioRootStorageLedger<'_>>(),
            size_of::<PrefixCost>(),
        )?,
        // The resident frame covers its disjoint Session/Editor/waveform
        // guards. The cache transaction is the remaining live writer body,
        // borrowed separately from that frame throughout prefix inspection.
        size_of::<crate::audio_transform::OriginalSamplerTransaction<'_>>(),
    )?;
    let retained_extra = add(add(cost.retained, backing)?, scratch)?;
    let peak_extra = add(add(cost.peak, backing)?, scratch)?;
    let retained = reservation
        .max_retained_bytes
        .checked_sub(retained_extra)
        .ok_or(AudioStorageError::ReservationExceeded)?;
    let peak = reservation
        .max_construction_peak_bytes
        .checked_sub(peak_extra)
        .ok_or(AudioStorageError::ReservationExceeded)?;
    AudioRootStorageLedger::arithmetic_cost_for_limits(reservation, retained, peak, visit)?;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
    visit(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    let actual = ledger.summary()?;
    if ledger.metadata_capacity_bytes() != Some(backing)
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
    Ok(())
}
#[expect(
    clippy::too_many_arguments,
    reason = "finite complete original-Sampler host operands"
)]
pub fn construct_pending_original_sampler_native_on_control<
    'owner,
    P: OriginalSamplerPendingSession<'owner>,
>(
    native: &mut dyn OriginalSamplerNativeHost,
    worker: &SamplerWorkerFields<'_>,
    current_source: &CapturedSourceFields<'_>,
    files: &dyn FileSlotsFields<'_>,
    families: &dyn SamplerFamilyInventoryFields,
    session: &mut P,
    previous: Option<&crate::AudioCompiledRevision>,
    reservation: AudioStorageLedgerReservation,
) -> Result<bool, AudioStorageError> {
    worker.require_current_constructor_owner()?;
    let actual_source = worker
        .current
        .source()
        .ok_or(AudioStorageError::SourceMismatch)?;
    let revision = worker.current.revision_field();
    let expected = session.pending_context_on_control()?;
    if !worker.current.body().contains(BodyRef::of(revision))
        || !actual_source.owner.body().same(current_source.owner.body())
        || expected.project_generation != current_source.payload.context().project_generation
        || revision.checked_add(1) != Some(expected.editor_revision)
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    let cost = metadata_cost(session, current_source, reservation)?;
    {
        let (resident, writers) = session.fields_and_writer_on_control();
        let inventory = writers.inventory();
        if !std::ptr::eq(resident.transforms(), inventory.preparation())
            || inventory.preparation().cancellation_requested_on_control()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let selected = native.selected_preparation_on_control()?;
        let preparation = selected.original_sampler_host_custody()?;
        let host = super::original_driver::OriginalSamplerConstructionHost {
            native,
            worker,
            source: current_source,
            files,
            families,
            previous,
        };
        preflight_prefix(
            &host,
            preparation,
            current_source,
            &resident,
            &inventory,
            &cost,
            reservation,
        )?;
    }
    // Every readonly projection/ledger has ended. SAME Session/native guards
    // stay held; this is the ONLY allocating producer call, with no callback,
    // exported result/quote or unlock/reacquire between preflight and mutation.
    if !session.stage_and_capture_on_control()? {
        return Ok(false);
    }
    let (resident, source, writers) = session.source_and_writer_on_control()?;
    super::original_driver::construct_staged_with_fresh_original(
        native,
        worker,
        &source,
        files,
        families,
        &resident,
        previous,
        writers,
        reservation,
    )?;
    Ok(true)
}
