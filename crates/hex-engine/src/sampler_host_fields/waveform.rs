//! Finite peak-pyramid fields used by the actual worker/family producers.
use super::{BodyRef, SharedMetadataRef, VecRef};
use crate::native_device_resources::OriginalSourceLease;
use crate::{Audio, AudioRootStorageLedger, AudioStorageError, OwnedAudioSource};

// Exactly the existing outer Vec and one inner Vec level. No recursive child
// callback or generic recursive ownership walk is available through this view.
trait LevelsLayout {
    fn outer(&self) -> VecRef<'_>;
    fn level(&self, index: usize) -> Result<VecRef<'_>, AudioStorageError>;
}
impl<T> LevelsLayout for Vec<Vec<T>> {
    fn outer(&self) -> VecRef<'_> {
        VecRef::of(self)
    }
    fn level(&self, index: usize) -> Result<VecRef<'_>, AudioStorageError> {
        self.get(index)
            .map(VecRef::of)
            .ok_or(AudioStorageError::SourceMismatch)
    }
}
#[derive(Clone, Copy)]
pub struct WaveformLevelsRef<'a> {
    levels: &'a dyn LevelsLayout,
}
impl<'a> WaveformLevelsRef<'a> {
    /// Physical capacities only. The audited waveform producer must separately
    /// establish that its peak elements have no owning children.
    pub fn of<T>(levels: &'a Vec<Vec<T>>) -> Self {
        Self { levels }
    }
}
pub enum WaveformSamples<'a> {
    None,
    Audio(&'a Audio),
    Original(&'a OriginalSourceLease),
    NativeOriginal(&'a OwnedAudioSource),
}
pub trait WaveformPayload {
    fn body(&self) -> BodyRef<'_>;
    fn bins(&self) -> VecRef<'_>;
    fn levels(&self) -> WaveformLevelsRef<'_>;
    fn samples(&self) -> WaveformSamples<'_>;
}
/// Actual paired outer field plus actual borrowed payload; no owner/receipt is
/// copied and neither footprint nor count creates publication/retirement proof.
pub struct WaveformFields<'a> {
    pub owner: SharedMetadataRef<'a>,
    pub payload: &'a dyn WaveformPayload,
}
pub(crate) fn visit_waveform<'a>(
    fields: &WaveformFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let WaveformFields { owner, payload } = fields;
    let body = payload.body();
    if !owner.payload().same(body) {
        return Err(AudioStorageError::SourceMismatch);
    }
    let bins = payload.bins();
    let levels = payload.levels().levels;
    let outer = levels.outer();
    if !body.contains(bins.body()) || !body.contains(outer.body()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    owner.observe(ledger)?;
    bins.observe(ledger)?;
    outer.observe(ledger)?;
    for index in 0..outer.len() {
        let level = levels.level(index)?;
        if !outer.element(index)?.same(level.body()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        level.observe(ledger)?;
    }
    match payload.samples() {
        WaveformSamples::None => Ok(()),
        WaveformSamples::Audio(audio) => {
            if !body.contains(BodyRef::of(audio)) {
                return Err(AudioStorageError::SourceMismatch);
            }
            ledger.embedded_native_audio(audio)
        }
        WaveformSamples::Original(source) => {
            if !body.contains(BodyRef::of(source)) {
                return Err(AudioStorageError::SourceMismatch);
            }
            ledger.embedded_original_source_lease(source)
        }
        WaveformSamples::NativeOriginal(source) => {
            if !body.contains(BodyRef::of(source)) {
                return Err(AudioStorageError::SourceMismatch);
            }
            ledger.embedded_native_source(source)
        }
    }
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual selected peak/PCM fields only; unknown children still refuse.
    pub fn borrow_sampler_waveform_fields_on_control(
        &mut self,
        fields: &WaveformFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_waveform(fields, ledger))
    }
}
