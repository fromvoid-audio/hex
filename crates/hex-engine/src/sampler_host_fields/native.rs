//! Finite guarded native fields. Producer validation is audited, not a public
//! capability. Unknown occupants must refuse rather than disappear as None.
use super::{BodyRef, SharedMetadataRef, VecRef};
use crate::{AudioCandidatePreparation, AudioRootStorageLedger, AudioStorageError};

pub struct NativeRuntimeFields<'a> {
    pub body: BodyRef<'a>,
    pub life: SharedMetadataRef<'a>,
    pub domains: VecRef<'a>,
    pub claims: [Option<&'a super::NativeClaimStorageRegistration>; 2],
    /// Actual guarded constructor-retained pair. Neither State footprint nor
    /// anchors alone certifies the other's descendants or native publication.
    pub transferred: Option<NativeTransferredRuntimeFields<'a>>,
}
pub struct NativeTransferredRuntimeFields<'a> {
    pub anchors: &'a crate::AudioCandidateAnchors,
    pub state: &'a super::NativeStateStorageRegistration,
}
pub struct NativePreparationFields<'a> {
    pub body: BodyRef<'a>,
    pub domains: VecRef<'a>,
    pub cell: &'a AudioCandidatePreparation,
}
/// All references remain inside the existing five native exclusions plus the inspection-only Resident exclusion. This
/// schema is not proof of absent descendants, retirement or publication.
pub trait NativeFields {
    fn root(&self) -> SharedMetadataRef<'_>;
    fn guarded_bodies(&self) -> [BodyRef<'_>; 5];
    fn outer_slots(&self) -> VecRef<'_>;
    fn outer_slot(&self, index: usize) -> Result<BodyRef<'_>, AudioStorageError>;
    fn root_fields(&self) -> Option<&super::NativeRootFieldRegistration>;
    fn spectrum_results(&self) -> &[Option<super::NativeSpectrumResultRegistration>];
    fn spectrum_packets(&self) -> &[Option<super::NativeSpectrumPacketRegistration>];
    fn spectrum_stop(&self) -> Option<&super::NativeAtomicSignalRegistration>;
    fn spectrum_rings(&self) -> [Option<&super::NativeFixedRingRegistration>; 6];
    fn spectrum_capture(&self) -> Option<&super::NativeSpectrumCapturedRegistration>;
    fn carriers(&self) -> [Option<&super::NativeCarrierStorageRegistration>; 4];
    fn prepared_rings(&self) -> [Option<&super::NativeFixedRingRegistration>; 9];
    fn atomic_signals(&self) -> [Option<&super::NativeAtomicSignalRegistration>; 8];
    fn status(&self) -> Result<Option<super::NativeStatusFields<'_>>, AudioStorageError>;
    fn resident(&self) -> Result<Option<super::NativeResidentFields<'_>>, AudioStorageError>;
    fn domain_signals(&self) -> Result<[SharedMetadataRef<'_>; 2], AudioStorageError>;
    fn runtime_slots(&self) -> VecRef<'_>;
    fn runtime(&self, index: usize) -> Result<NativeRuntimeFields<'_>, AudioStorageError>;
    fn archives(&self) -> VecRef<'_>;
    /// Actual preparations, anchors, executions, failed payloads and bank-drop
    /// backings, in this fixed order. Every occupied child is mandatory below.
    fn work_backings(&self) -> [VecRef<'_>; 5];
    fn preparation(
        &self,
        index: usize,
    ) -> Result<Option<NativePreparationFields<'_>>, AudioStorageError>;
    fn validate_remaining_children(&self) -> Result<(), AudioStorageError>;
}
fn contains(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    parent
        .contains(child)
        .then_some(())
        .ok_or(AudioStorageError::SourceMismatch)
}
pub(crate) fn visit_native<'a>(
    fields: &'a dyn NativeFields,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    visit_native_at(fields, None, ledger)
}
/// Only the closed compiler may compose the SAME selected constructor custody
/// with its mandatory inner Inputs. Every other preparation stays whole.
pub(crate) fn visit_native_for_original_sampler<'a>(
    fields: &'a dyn super::OriginalSamplerNativeHost,
    selected: &'a AudioCandidatePreparation,
    preparation: &'a crate::AudioPreparationCustody,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    selected.require_original_sampler_constructor_custody(preparation)?;
    visit_native_at(fields, Some((selected, preparation)), ledger)
}
fn visit_native_at<'a>(
    fields: &'a dyn NativeFields,
    selected: Option<(
        &'a AudioCandidatePreparation,
        &'a crate::AudioPreparationCustody,
    )>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    fields.validate_remaining_children()?;
    let root = fields.root();
    root.observe(ledger)?;
    if let Some(fields) = fields.root_fields() {
        contains(root.payload(), BodyRef::of(fields))?;
        fields.visit_fields(root, ledger)?;
    }
    let [bank, execution, returns, native_bank, native_work] = fields.guarded_bodies();
    for body in [bank, execution, returns, native_bank, native_work] {
        contains(root.payload(), body)?;
    }
    if let Some(resident) = fields.resident()? {
        contains(root.payload(), resident.body)?;
        crate::audio_storage::native_resident::visit_resident(resident, ledger)?;
    }
    let slots = fields.outer_slots();
    contains(bank, slots.body())?;
    slots.observe(ledger)?;
    for index in 0..slots.len() {
        if !slots.element(index)?.same(fields.outer_slot(index)?) {
            return Err(AudioStorageError::SourceMismatch);
        }
    }
    for signal in fields.domain_signals()? {
        contains(native_bank, signal.body())?;
        signal.observe(ledger)?;
    }
    for result in fields.spectrum_results().iter().flatten() {
        contains(native_bank, BodyRef::of(result))?;
        result.visit_fields(ledger)?;
    }
    for packet in fields.spectrum_packets().iter().flatten() {
        contains(native_bank, BodyRef::of(packet))?;
        packet.visit_fields(ledger)?;
    }
    if let Some(stop) = fields.spectrum_stop() {
        contains(native_bank, BodyRef::of(stop))?;
        stop.visit_fields(ledger)?;
    }
    for ring in fields.spectrum_rings().into_iter().flatten() {
        contains(native_bank, BodyRef::of(ring))?;
        ring.visit_fields(ledger)?;
    }
    if let Some(capture) = fields.spectrum_capture() {
        contains(native_bank, BodyRef::of(capture))?;
        capture.visit_fields(ledger)?;
    }
    for carrier in fields.carriers().into_iter().flatten() {
        contains(native_bank, BodyRef::of(carrier))?;
        carrier.visit_fields(ledger)?;
    }
    for ring in fields.prepared_rings().into_iter().flatten() {
        contains(native_bank, BodyRef::of(ring))?;
        ring.visit_fields(ledger)?;
    }
    for signal in fields.atomic_signals().into_iter().flatten() {
        contains(native_bank, BodyRef::of(signal))?;
        signal.visit_fields(ledger)?;
    }
    if let Some(status) = fields.status()? {
        contains(native_bank, status.body)?;
        crate::audio_storage::native_status::visit_status(status, ledger)?;
    }
    let slots = fields.runtime_slots();
    contains(native_bank, slots.body())?;
    slots.observe(ledger)?;
    let archives = fields.archives();
    contains(native_bank, archives.body())?;
    archives.observe(ledger)?;
    for index in 0..slots.len() {
        let runtime = fields.runtime(index)?;
        if !slots.element(index)?.same(runtime.body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        contains(runtime.body, runtime.life.body())?;
        contains(runtime.body, runtime.domains.body())?;
        runtime.life.observe(ledger)?;
        runtime.domains.observe(ledger)?;
        for claim in runtime.claims.into_iter().flatten() {
            claim.visit_fields(root, slots, index, runtime.life, ledger)?;
        }
        if let Some(transferred) = runtime.transferred {
            contains(runtime.body, BodyRef::of(transferred.anchors))?;
            contains(runtime.body, BodyRef::of(transferred.state))?;
            if !transferred.state.is_live_on_control() {
                return Err(AudioStorageError::UnknownStorage);
            }
            transferred.state.visit_fields(ledger)?;
            // Direct mandatory traversal inside this ONE outer mutation. A
            // public ledger wrapper here would recursively finish a ledger
            // whose complete native input walk is still in progress.
            transferred.anchors.visit_transferred_graph_fields(ledger)?;
        }
    }
    let backings = fields.work_backings();
    for backing in backings {
        contains(native_work, backing.body())?;
        backing.observe(ledger)?;
    }
    let mut selected_seen = false;
    for index in 0..backings[0].len() {
        if let Some(preparation) = fields.preparation(index)? {
            contains(backings[0].element(index)?, preparation.body)?;
            contains(preparation.body, preparation.domains.body())?;
            contains(preparation.body, BodyRef::of(preparation.cell))?;
            preparation.domains.observe(ledger)?;
            if let Some((cell, custody)) =
                selected.filter(|(cell, _)| std::ptr::eq(*cell, preparation.cell))
            {
                if selected_seen {
                    return Err(AudioStorageError::SourceMismatch);
                }
                selected_seen = true;
                cell.visit_original_sampler_constructor_shell(custody, ledger)?;
            } else {
                preparation.cell.visit_sampler_host_children(ledger)?;
            }
        }
    }
    if selected.is_some() && !selected_seen {
        return Err(AudioStorageError::SourceMismatch);
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Guarded actual descendants only. Failure invalidates this entire ledger;
    /// no partially visited prefix may become an approval or be resumed.
    pub fn borrow_sampler_native_fields_on_control(
        &mut self,
        fields: &'a dyn NativeFields,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_native(fields, ledger))
    }
}
