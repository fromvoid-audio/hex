//! Actual snapshot-family descendants, not native source/publication authority.
use super::{BodyRef, CapturedSourceFields, SharedMetadataRef, VecRef, WaveformFields};
use crate::{
    AudioRootStorageLedger, AudioStorageError, SharedGainAutomationPreview, SharedMetadata,
    SharedMidiArrangementPreview,
};
use std::path::PathBuf;

pub trait SnapshotViewPayload {
    fn body(&self) -> BodyRef<'_>;
    fn source(&self) -> Option<CapturedSourceFields<'_>>;
    fn comparison_source(&self) -> Option<CapturedSourceFields<'_>>;
    fn project(&self) -> &SharedMetadata<hex_project::Project>;
    fn waveforms(&self) -> VecRef<'_>;
    fn waveform(&self, index: usize) -> Result<WaveformFields<'_>, AudioStorageError>;
    fn clip_waveforms(&self) -> VecRef<'_>;
    fn clip_waveform(&self, index: usize) -> Result<WaveformFields<'_>, AudioStorageError>;
    fn midi_preview(&self) -> &SharedMidiArrangementPreview;
    fn gain_preview(&self) -> Option<&SharedGainAutomationPreview>;
    fn document_path(&self) -> Option<&PathBuf>;
}
pub struct SnapshotViewFields<'a> {
    pub owner: SharedMetadataRef<'a>,
    pub payload: &'a dyn SnapshotViewPayload,
}
pub(crate) fn visit_snapshot_children<'a>(
    payload: &'a dyn SnapshotViewPayload,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let body = payload.body();
    for source in [payload.source(), payload.comparison_source()]
        .into_iter()
        .flatten()
    {
        if !body.contains(source.owner.body()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        super::captured_source::visit_captured_source(&source, ledger)?;
    }
    let project = payload.project();
    if !body.contains(BodyRef::of(project)) {
        return Err(AudioStorageError::SourceMismatch);
    }
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
    let waveforms = payload.waveforms();
    if !body.contains(waveforms.body()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    waveforms.observe(ledger)?;
    for index in 0..waveforms.len() {
        let waveform = payload.waveform(index)?;
        if !waveforms.element(index)?.contains(waveform.owner.body()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        super::waveform::visit_waveform(&waveform, ledger)?;
    }
    let clips = payload.clip_waveforms();
    if !body.contains(clips.body()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    clips.observe(ledger)?;
    for index in 0..clips.len() {
        let waveform = payload.clip_waveform(index)?;
        if !clips.element(index)?.contains(waveform.owner.body()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        super::waveform::visit_waveform(&waveform, ledger)?;
    }
    let midi = payload.midi_preview();
    if !body.contains(BodyRef::of(midi)) {
        return Err(AudioStorageError::SourceMismatch);
    }
    crate::midi_preview::visit_shared_midi_preview_storage_on_control_inner(midi, ledger)?;
    if let Some(gain) = payload.gain_preview() {
        if !body.contains(BodyRef::of(gain)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        crate::gain_envelope::visit_shared_gain_preview_storage_on_control_inner(gain, ledger)?;
    }
    if let Some(path) = payload.document_path() {
        if !body.contains(BodyRef::of(path)) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.path_backing(path)?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Only children of the actual view. In particular a legacy raw outer Arc
    /// is NOT qualified or charged by visiting its supported descendants.
    pub fn borrow_sampler_snapshot_children_fields_on_control(
        &mut self,
        payload: &'a dyn SnapshotViewPayload,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_snapshot_children(payload, ledger))
    }
    /// Paired real family record field plus its exact actual local payload.
    /// This supplies no transport/root/native retirement authority.
    pub fn borrow_sampler_snapshot_view_fields_on_control(
        &mut self,
        fields: &SnapshotViewFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            if !fields.owner.payload().same(fields.payload.body()) {
                return Err(AudioStorageError::SourceMismatch);
            }
            fields.owner.observe(ledger)?;
            visit_snapshot_children(fields.payload, ledger)
        })
    }
}
