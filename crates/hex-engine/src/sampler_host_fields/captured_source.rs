use super::{BodyRef, SharedMetadataRef, VecRef};
use crate::{Audio, AudioRootStorageLedger, AudioStorageError, SharedMetadata};

/// Audited local canonical-source payload. Every actual binding is required;
/// verified/unknown source storage refuses, never a missing optional record.
pub trait CapturedSourcePayload {
    fn context(&self) -> crate::AudioSourceContext;
    fn body(&self) -> BodyRef<'_>;
    fn project(&self) -> &SharedMetadata<hex_project::Project>;
    fn bindings(&self) -> VecRef<'_>;
    fn resident(&self, index: usize) -> Result<&Audio, AudioStorageError>;
}
/// Per-index borrowed record without an allocated projection list. The actual
/// paired field AND its payload remain borrowed, not cloned or adopted.
pub struct CapturedSourceFields<'a> {
    pub owner: SharedMetadataRef<'a>,
    pub payload: &'a dyn CapturedSourcePayload,
}
pub(crate) fn visit_captured_source<'a>(
    fields: &CapturedSourceFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let CapturedSourceFields { owner, payload } = fields;
    let body = payload.body();
    let project = payload.project();
    let bindings = payload.bindings();
    if !owner.payload().same(body)
        || !body.contains(BodyRef::of(project))
        || !body.contains(bindings.body())
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
        let audio = payload.resident(index)?;
        if !bindings.element(index)?.contains(BodyRef::of(audio)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.embedded_native_audio(audio)?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected real source fields only, not worker closure/publication.
    pub fn borrow_sampler_captured_source_fields_on_control(
        &mut self,
        fields: &CapturedSourceFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_captured_source(fields, ledger))
    }
}
