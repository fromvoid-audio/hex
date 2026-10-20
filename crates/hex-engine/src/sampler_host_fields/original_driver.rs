//! Borrowed interface for the ONE private Audio selected-parent facade.
//! This is an audited internal call boundary, not a hostile-client capability.
use super::{BodyRef, NativeFields};
use crate::{AudioCandidatePreparation, AudioCandidatePreparationOperations, AudioStorageError};

/// Fresh callback-local input ledger, scoped inside this immutable native loan.
/// Mandatory typed walks establish every row; this never imports arbitrary
/// caller rows or treats the absence of origin tags as absence of old owners.
fn fresh_original<'a>(
    host: &'a OriginalSamplerConstructionHost<'a, '_>,
    resident: &'a dyn super::OriginalSamplerResidentFields,
    inventory: &'a crate::audio_transform::AudioTransformPcmInventory<'a>,
    reservation: crate::AudioStorageLedgerReservation,
) -> Result<crate::AudioRootStorageLedger<'a>, AudioStorageError> {
    let selected = host.native.selected_preparation_on_control()?;
    let preparation = selected.original_sampler_host_custody()?;
    let visit = |ledger: &mut crate::AudioRootStorageLedger<'a>| {
        host.visit(preparation, Some(host.source.payload.project()), ledger)?;
        super::visit_original_sampler_resident(resident, inventory, ledger)
    };
    let backing = crate::AudioRootStorageLedger::requested_metadata_bytes(reservation)?;
    let extra = backing
        .checked_add(size_of::<crate::AudioRootStorageLedger<'_>>())
        .ok_or(AudioStorageError::Overflow)?;
    let retained = reservation
        .max_retained_bytes
        .checked_sub(extra)
        .ok_or(AudioStorageError::ReservationExceeded)?;
    let peak = reservation
        .max_construction_peak_bytes
        .checked_sub(extra)
        .ok_or(AudioStorageError::ReservationExceeded)?;
    crate::AudioRootStorageLedger::arithmetic_cost_for_limits(reservation, retained, peak, visit)?;
    let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(reservation)?;
    visit(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    if ledger.metadata_capacity_bytes() != Some(backing) {
        return Err(AudioStorageError::ReservationExceeded);
    }
    Ok(ledger)
}

#[expect(
    clippy::too_many_arguments,
    reason = "same mandatory finite actual operands"
)]
pub(super) fn construct_staged_with_fresh_original(
    native: &mut dyn OriginalSamplerNativeHost,
    worker: &super::SamplerWorkerFields<'_>,
    source: &super::CapturedSourceFields<'_>,
    files: &dyn super::FileSlotsFields<'_>,
    families: &dyn super::SamplerFamilyInventoryFields,
    resident: &dyn super::OriginalSamplerResidentFields,
    previous: Option<&crate::AudioCompiledRevision>,
    writers: &mut crate::audio_transform::OriginalSamplerTransaction<'_>,
    reservation: crate::AudioStorageLedgerReservation,
) -> Result<(), AudioStorageError> {
    let target = source.payload.project();
    let previous_contexts = previous
        .map(crate::AudioCompiledRevision::original_sampler_previous_contexts)
        .transpose()?;
    let producer = crate::native_device_resources::OriginalResourceBankPlan::borrow_resident_fields_on_control(resident, target)
        .map_err(|_| AudioStorageError::UnknownStorage)?;
    let construction = {
        let inventory = writers.inventory();
        let host = OriginalSamplerConstructionHost {
            native,
            worker,
            source,
            files,
            families,
            previous,
        };
        let original = fresh_original(&host, resident, &inventory, reservation)?;
        native
            .selected_preparation_on_control()?
            .inspect_original_sampler_with_writers_on_control(
                Some(&host),
                target,
                &producer,
                Some(resident),
                resident.editor(),
                previous_contexts,
                &original,
                writers,
                reservation,
                crate::audio_candidate::ConstructorPurpose::NativeFresh,
            )?
    };
    // Both complete input ledgers and all native readonly projections ended;
    // constructor immediately reborrows the SAME physical persistent parent.
    native
        .selected_operations_on_control()?
        .construct_native_sampler_in_host(target, previous_contexts, writers, construction)?;
    for _ in 0..2 {
        let inspection = {
            let inventory = writers.inventory();
            let host = OriginalSamplerConstructionHost {
                native,
                worker,
                source,
                files,
                families,
                previous,
            };
            let original = fresh_original(&host, resident, &inventory, reservation)?;
            // Actual new constructor-owned fields are walked with their real
            // origin partitions BEFORE existing original_ledger_storage imports
            // observations. Old/transferred owners retain their actual custody;
            // this local baseline does not create, erase or adopt origins.
            native
                .selected_preparation_on_control()?
                .inspect_native_sampler_seal_in_host(
                    &host,
                    resident,
                    &producer,
                    previous_contexts,
                    &original,
                    writers,
                    reservation,
                )?
        };
        if native
            .selected_operations_on_control()?
            .install_native_sampler_seal_in_host(inspection)?
        {
            return Ok(());
        }
    }
    Err(AudioStorageError::UnknownStorage)
}

/// Engine's closed original-Sampler operation may inspect the actual complete
/// host, end ALL readonly projections/ledgers, then reborrow operations on the
/// SAME persistent cell. The concrete facade and every native guard stay in
/// Audio. No construction value, ledger, owner or approval crosses back.
///
/// Calling this interface alone grants no admission. The complete constructor
/// must visit mandatory worker/family/File/current/captured/native inputs and
/// check normal binding and both caps before borrowing mutable operations.
/// Repository policy restricts this to the actual private Audio implementation.
/// ```compile_fail
/// use hex_engine::sampler_host_fields::OriginalSamplerNativeHost;
/// fn extract(host: &mut dyn OriginalSamplerNativeHost)
///     -> &mut hex_engine::AudioCandidatePreparation {
///     host.selected_operations_on_control().unwrap()
/// }
/// ```
pub trait OriginalSamplerNativeHost: NativeFields {
    /// The real temporary facade/guards, footprint only; descendants still use
    /// NativeFields and the mandatory enclosing-input schemas.
    fn construction_frame(&self) -> BodyRef<'_>;
    fn selected_preparation_on_control(
        &self,
    ) -> Result<&AudioCandidatePreparation, AudioStorageError>;
    /// No mutation is possible until overlapping immutable loans have ended.
    /// Audio must revalidate the actual token and persistent field correlation.
    fn selected_operations_on_control(
        &mut self,
    ) -> Result<AudioCandidatePreparationOperations<'_>, AudioStorageError>;
}

/// Closed original-class constructor for the actual guarded Audio destination.
/// The audited caller retains its real worker/family and Session exclusions;
/// Audio retains its six native exclusions throughout this call. Neither the
/// supplied original ledger nor any returned result grants publication.
/// Every original row must bind to the actual mandatory operands on BOTH walks.
/// Backend/DSP/File operations require all these loans to have ended first.
#[expect(
    clippy::too_many_arguments,
    reason = "finite mandatory actual host operands"
)]
pub fn construct_original_sampler_native_with_host_on_control(
    native: &mut dyn OriginalSamplerNativeHost,
    worker: &super::SamplerWorkerFields<'_>,
    source: &super::CapturedSourceFields<'_>,
    files: &dyn super::FileSlotsFields<'_>,
    families: &dyn super::SamplerFamilyInventoryFields,
    resident: &dyn super::OriginalSamplerResidentFields,
    producer: &crate::native_device_resources::OriginalResourceBankPlan<'_>,
    previous: Option<&crate::AudioCompiledRevision>,
    original: &crate::AudioRootStorageLedger<'_>,
    writers: &mut crate::audio_transform::OriginalSamplerTransaction<'_>,
    reservation: crate::AudioStorageLedgerReservation,
) -> Result<(), AudioStorageError> {
    if !std::ptr::eq(resident.transforms(), writers.preparation()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    let previous_contexts = previous
        .map(crate::AudioCompiledRevision::original_sampler_previous_contexts)
        .transpose()?;
    let target = source.payload.project();
    let construction = {
        let host = OriginalSamplerConstructionHost {
            worker,
            source,
            files,
            families,
            native,
            previous,
        };
        native
            .selected_preparation_on_control()?
            .inspect_original_sampler_with_writers_on_control(
                Some(&host),
                target,
                producer,
                Some(resident),
                resident.editor(),
                previous_contexts,
                original,
                writers,
                reservation,
                crate::audio_candidate::ConstructorPurpose::NativeFresh,
            )?
    };
    // NO overlapping immutable projection or ledger survives this point. This
    // operation rechecks the SAME physical parent under the original guards.
    native
        .selected_operations_on_control()?
        .construct_native_sampler_in_host(target, previous_contexts, writers, construction)?;
    // A fresh inspection follows physical-capture installation: that new field
    // must itself be inventoried. This is at most two fixed metadata phases,
    // never a retry after failed construction or consumed cleanup.
    for _ in 0..2 {
        let inspection = {
            let host = OriginalSamplerConstructionHost {
                worker,
                source,
                files,
                families,
                native,
                previous,
            };
            native
                .selected_preparation_on_control()?
                .inspect_native_sampler_seal_in_host(
                    &host,
                    resident,
                    producer,
                    previous_contexts,
                    original,
                    writers,
                    reservation,
                )?
        };
        if native
            .selected_operations_on_control()?
            .install_native_sampler_seal_in_host(inspection)?
        {
            return Ok(());
        }
    }
    Err(AudioStorageError::UnknownStorage)
}

/// Engine-private borrowed composition. It cannot cross the immutable phase or
/// become constructor authority: the compiler still performs BOTH passes and
/// caps, then drops every borrowed projection before same-parent mutation.
/// Normally-present unsupported worker roots remain the owning producer's
/// explicit refusal; these child fields do not bless legacy outer Arcs.
pub(crate) struct OriginalSamplerConstructionHost<'a, 'files: 'a> {
    pub(crate) worker: &'a super::SamplerWorkerFields<'a>,
    pub(crate) source: &'a super::CapturedSourceFields<'a>,
    pub(crate) files: &'a dyn super::FileSlotsFields<'files>,
    pub(crate) families: &'a dyn super::SamplerFamilyInventoryFields,
    pub(crate) native: &'a dyn OriginalSamplerNativeHost,
    pub(crate) previous: Option<&'a crate::AudioCompiledRevision>,
}
impl<'a, 'files: 'a> OriginalSamplerConstructionHost<'a, 'files> {
    pub(crate) fn visit<'scope>(
        &'scope self,
        preparation: &'scope crate::AudioPreparationCustody,
        target: Option<&'scope crate::SharedMetadata<hex_project::Project>>,
        ledger: &mut crate::AudioRootStorageLedger<'scope>,
    ) -> Result<(), AudioStorageError>
    where
        'a: 'scope,
    {
        let target = target.ok_or(AudioStorageError::SourceMismatch)?;
        if !std::ptr::eq(target, self.source.payload.project()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        let selected = self.native.selected_preparation_on_control()?;
        selected.require_original_sampler_constructor_custody(preparation)?;
        for frame in [
            BodyRef::of(self),
            BodyRef::of(self.worker),
            BodyRef::of(self.source),
            self.files.borrowed_frame(),
            self.native.construction_frame(),
        ] {
            ledger.bytes(
                frame.owner(),
                crate::audio_storage::Location("original-native-host-frame", 0),
                frame.layout_bytes(),
            )?;
        }
        if let Some(previous) = self.previous {
            previous.visit_shared_storage_on_control(ledger)?;
        }
        super::worker::visit_worker(self.worker, ledger)?;
        super::captured_source::visit_captured_source(self.source, ledger)?;
        super::file_slots::visit_file_slots(self.files, ledger)?;
        super::family_inventory::visit_family_inventory(self.families, ledger)?;
        super::native::visit_native_for_original_sampler(self.native, selected, preparation, ledger)
    }
}
