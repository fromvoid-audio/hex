use super::{BodyRef, SharedMetadataRef, VecRef};
use crate::{Audio, AudioRootStorageLedger, AudioStorageError, SharedMetadata};

/// Audited canonical-source producer. Every actual binding entry is required;
/// a verified/unknown source returns refusal, never a missing optional record.
pub trait CapturedSourceFields<'a> {
    fn owner(&self) -> SharedMetadataRef<'a>;
    fn project(&self) -> &'a SharedMetadata<hex_project::Project>;
    fn bindings(&self) -> VecRef<'a>;
    fn resident(&self, index: usize) -> Result<&'a Audio, AudioStorageError>;
}
pub(crate) fn visit_captured_source<'a>(
    fields: &dyn CapturedSourceFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let owner = fields.owner();
    let project = fields.project();
    let bindings = fields.bindings();
    // Actual paired outer payload contains these actual fields. Equality of
    // models/contexts or a scalar counter grants no source association.
    if !owner.payload().contains(BodyRef::of(project)) || !owner.payload().contains(bindings.body())
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    owner.observe(ledger)?;
    SharedMetadataRef::of(project).observe(ledger)?;
    if project.import_provenance().is_some()
        || project
            .tracks()
            .iter()
            .any(|t| t.clips().iter().any(|c| c.audio_edit().is_some()))
    {
        return Err(AudioStorageError::UnknownStorage);
    }
    ledger.project_exclusive_buffers(project)?;
    ledger.project_midi_sources(project)?;
    ledger.project_native_states(project)?;
    bindings.observe(ledger)?;
    for index in 0..bindings.len() {
        let audio = fields.resident(index)?;
        if !bindings.element(index)?.contains(BodyRef::of(audio)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.embedded_native_audio(audio)?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// The selected real source fields only, not worker closure/publication.
    pub fn borrow_sampler_captured_source_fields_on_control(
        &mut self,
        fields: &dyn CapturedSourceFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_captured_source(fields, ledger))
    }
}
