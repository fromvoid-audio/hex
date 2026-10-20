//! Actual Session fields under existing waveform/cache writers. Finite schema,
//! not a callback, source adoption, owner registry or completeness certificate.
use super::{BodyRef, VecRef, WaveformFields};
use crate::{Audio, AudioRootStorageLedger, AudioStorageError, OwnedAudioSource};

pub enum ResidentOriginalFields<'a> {
    Stereo(&'a Audio),
    Mono(&'a OwnedAudioSource),
}
pub struct ResidentAssetFields<'a> {
    pub asset: hex_project::AssetId,
    pub body: BodyRef<'a>,
    pub original_body: BodyRef<'a>,
    pub original: ResidentOriginalFields<'a>,
    pub rendered: Option<&'a Audio>,
    pub waveform: WaveformFields<'a>,
}
pub struct ResidentClipWaveformFields<'a> {
    pub body: BodyRef<'a>,
    pub waveform: WaveformFields<'a>,
}
pub trait OriginalSamplerResidentFields {
    fn body(&self) -> BodyRef<'_>;
    fn frame(&self) -> [BodyRef<'_>; 5];
    fn editor(&self) -> &hex_project::Editor;
    fn pending_editor(&self)
    -> Option<crate::audio_storage::OriginalSamplerPendingModelFields<'_>>;
    fn pending_source(&self) -> Result<OriginalSamplerPendingSourceFields<'_>, AudioStorageError>;
    fn transforms(&self) -> &crate::audio_transform::AudioTransformPreparation;
    fn cancellation(&self) -> Result<&crate::RetainedControlCancellation, AudioStorageError>;
    fn sine(&self) -> &crate::SharedSineTable;
    fn media(&self) -> VecRef<'_>;
    fn asset(&self, index: usize) -> Result<ResidentAssetFields<'_>, AudioStorageError>;
    fn waveforms(&self) -> VecRef<'_>;
    fn clip_waveform(
        &self,
        index: usize,
    ) -> Result<ResidentClipWaveformFields<'_>, AudioStorageError>;
    fn verified_sources(&self) -> VecRef<'_>;
    fn validate_remaining_fields(&self) -> Result<(), AudioStorageError>;
}
/// Actual persistent pending metadata fields, supplied by the sole Session
/// producer under its single held guard. None is valid only for a proved
/// vacant/requested phase; phase and correlation are checked by that producer.
pub struct OriginalSamplerPendingSourceFields<'a> {
    pub body: BodyRef<'a>,
    pub project: Option<&'a crate::SharedMetadata<hex_project::Project>>,
    pub source: Option<super::CapturedSourceFields<'a>>,
}
pub(crate) fn visit_resident_editor<'a>(
    fields: &'a dyn OriginalSamplerResidentFields,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let editor = fields.editor();
    if let Some(pending) = fields.pending_editor() {
        if !std::ptr::eq(editor, pending.editor()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.pending_model_fields(pending)
    } else {
        ledger.borrow_editor_history_buffers_on_control(editor)?;
        ledger.borrow_editor_exclusive_buffers_on_control(editor)?;
        ledger.borrow_editor_midi_sources_on_control(editor)?;
        ledger.borrow_editor_native_states_on_control(editor)
    }
}
fn contains(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    parent
        .contains(child)
        .then_some(())
        .ok_or(AudioStorageError::SourceMismatch)
}
pub(crate) fn visit_resident<'a>(
    fields: &'a dyn OriginalSamplerResidentFields,
    inventory: &'a crate::audio_transform::AudioTransformPcmInventory<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    fields.validate_remaining_fields()?;
    if !std::ptr::eq(fields.transforms(), inventory.preparation()) {
        return Err(AudioStorageError::SourceMismatch);
    }

    for frame in fields.frame() {
        ledger.bytes(
            frame.owner(),
            crate::audio_storage::Location("original-sampler-loan-frame", 0),
            frame.layout_bytes(),
        )?;
    }
    let body = fields.body();
    // Footprint only. Owning children and their actual constructor lineage are
    // independently mandatory below; no root/retirement authority is issued.
    ledger.bytes(
        body.owner(),
        crate::audio_storage::Location("root-inline", 0),
        body.layout_bytes(),
    )?;
    contains(body, BodyRef::of(fields.editor()))?;
    contains(body, BodyRef::of(fields.transforms()))?;
    contains(body, BodyRef::of(fields.sine()))?;
    contains(body, BodyRef::of(fields.cancellation()?))?;
    let media = fields.media();
    let waveforms = fields.waveforms();
    let verified = fields.verified_sources();
    for backing in [media, waveforms, verified] {
        contains(body, backing.body())?;
        backing.observe(ledger)?;
    }
    // Verified file descriptors have owning children not yet in this original
    // resident class. Their ordinary production route remains an open class.
    if !verified.is_empty() {
        return Err(AudioStorageError::UnknownStorage);
    }
    fields.cancellation()?.visit_storage_on_control(ledger)?;
    fields.sine().visit_storage_on_control(ledger)?;
    visit_resident_editor(fields, ledger)?;
    let pending = fields.pending_source()?;
    contains(body, pending.body)?;
    if let Some(project) = pending.project {
        contains(pending.body, BodyRef::of(project))?;
        ledger.borrow_shared_metadata_body_on_control(project)?;
        ledger.borrow_project_exclusive_buffers_on_control(project)?;
        ledger.borrow_project_midi_sources_on_control(project)?;
        ledger.borrow_project_native_states_on_control(project)?;
    }
    if let Some(source) = &pending.source {
        contains(pending.body, source.owner.body())?;
        super::captured_source::visit_captured_source(source, ledger)?;
    }
    inventory.visit_original_only_control_storage_on_control(ledger)?;

    for index in 0..media.len() {
        let asset = fields.asset(index)?;
        contains(media.element(index)?, asset.body)?;
        contains(asset.body, asset.original_body)?;
        contains(asset.body, asset.waveform.owner.body())?;
        match asset.original {
            ResidentOriginalFields::Stereo(audio) => {
                contains(asset.original_body, BodyRef::of(audio))?;
                ledger.borrow_embedded_native_audio_on_control(audio)?;
            }
            ResidentOriginalFields::Mono(source) => {
                contains(asset.original_body, BodyRef::of(source))?;
                ledger.borrow_embedded_native_source_on_control(source)?;
            }
        }
        if let Some(audio) = asset.rendered {
            contains(asset.body, BodyRef::of(audio))?;
            ledger.borrow_embedded_native_audio_on_control(audio)?;
        }
        super::waveform::visit_waveform(&asset.waveform, ledger)?;
    }
    for index in 0..waveforms.len() {
        let clip = fields.clip_waveform(index)?;
        contains(waveforms.element(index)?, clip.body)?;
        contains(clip.body, clip.waveform.owner.body())?;
        super::waveform::visit_waveform(&clip.waveform, ledger)?;
    }
    Ok(())
}
