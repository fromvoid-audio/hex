//! Actual existing outer lifecycle slots and inner Session exclusions. This is
//! a finite borrowed schema, not a callback, owner registry or admission token.
use super::{BodyRef, FamilyCompiledRecordFields, FamilyExportResultFields, FamilyFields, VecRef};
use crate::{
    AudioRootStorageLedger, AudioStorageError, RetainedControlCancellationSignal,
    RetainedControlJob,
};

pub struct SamplerOuterFamilySlotFields<'a> {
    pub body: BodyRef<'a>,
    pub cancellation: &'a RetainedControlCancellationSignal,
    pub job: Option<&'a RetainedControlJob>,
}
/// The actual Audio-owned static table and its held read guards. Every slot,
/// including vacant ones, is required. Bodies never certify unseen children.
pub trait SamplerOuterFamilyFields {
    fn body(&self) -> BodyRef<'_>;
    fn table(&self) -> BodyRef<'_>;
    fn slots(&self) -> VecRef<'_>;
    fn guards(&self) -> VecRef<'_>;
    fn slot(&self, index: usize) -> Result<SamplerOuterFamilySlotFields<'_>, AudioStorageError>;
}
pub struct SamplerFamilyGuardFields<'a> {
    pub guard: BodyRef<'a>,
    pub owner: BodyRef<'a>,
    pub records: &'a dyn FamilyFields,
    pub retiring_compiled: FamilyCompiledRecordFields<'a>,
    pub retiring_export: FamilyExportResultFields<'a>,
}
/// ONE actual Desktop adapter with outer-before-inner guards held. No allocated
/// projection list or caller-supplied visitor is accepted by Engine.
pub trait SamplerFamilyInventoryFields {
    fn body(&self) -> BodyRef<'_>;
    fn outer(&self) -> &dyn SamplerOuterFamilyFields;
    fn guards(&self) -> VecRef<'_>;
    fn family(&self, index: usize) -> Result<SamplerFamilyGuardFields<'_>, AudioStorageError>;
}
fn contains(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    parent
        .contains(child)
        .then_some(())
        .ok_or(AudioStorageError::SourceMismatch)
}
fn footprint<'a>(
    body: BodyRef<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    ledger.bytes(
        body.owner(),
        crate::audio_storage::Location("root-inline", 0),
        body.layout_bytes(),
    )
}
pub(crate) fn visit_family_inventory<'a>(
    fields: &'a dyn SamplerFamilyInventoryFields,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    footprint(fields.body(), ledger)?;
    let outer = fields.outer();
    footprint(outer.body(), ledger)?;
    footprint(outer.table(), ledger)?;
    let slots = outer.slots();
    let outer_guards = outer.guards();
    let inner_guards = fields.guards();
    contains(outer.table(), slots.body())?;
    contains(outer.body(), outer_guards.body())?;
    if slots.len() != outer_guards.len() {
        return Err(AudioStorageError::SourceMismatch);
    }
    slots.observe(ledger)?;
    outer_guards.observe(ledger)?;
    inner_guards.observe(ledger)?;
    let mut inner_index = 0;
    for index in 0..slots.len() {
        let slot = outer.slot(index)?;
        if !slots.element(index)?.same(slot.body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        contains(slot.body, BodyRef::of(slot.cancellation))?;
        slot.cancellation.visit_family_field_on_control(ledger)?;
        if let Some(job) = slot.job {
            contains(slot.body, BodyRef::of(job))?;
            // Constructor-owned dynamic Box and its ACTUAL concrete payload
            // body, not address/counters supplied by the outer adapter.
            job.visit_outer_fields_on_control(ledger)?;
            let family = fields.family(inner_index)?;
            if !inner_guards.element(inner_index)?.same(family.guard)
                || !job.matches_actual_payload_body(family.owner)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            contains(family.owner, family.records.body())?;
            super::family::visit_family(family.records, ledger)?;
            contains(family.owner, family.retiring_compiled.body)?;
            if let Some(reader) = family.retiring_compiled.reader {
                contains(family.retiring_compiled.body, BodyRef::of(reader))?;
                reader.visit_shared_storage_on_control(ledger)?;
            }
            contains(family.owner, family.retiring_export.body)?;
            super::family::visit_export_result(family.retiring_export, ledger)?;
            inner_index = inner_index
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
        }
    }
    if inner_index != inner_guards.len() {
        return Err(AudioStorageError::SourceMismatch);
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Existing guarded family DATA only. Worker/native/File/source children,
    /// beforebirth scratch and final publication remain mandatory separately.
    pub fn borrow_sampler_family_inventory_fields_on_control(
        &mut self,
        fields: &'a dyn SamplerFamilyInventoryFields,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_family_inventory(fields, ledger))
    }
}
