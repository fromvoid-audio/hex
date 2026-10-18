//! One concrete completed waveform in the actual pending sample owner.
use super::*;
impl PreparedDeviceSample {
    /// Resolve the SAME frozen assignment/original used by resource planning,
    /// then retain the one completed paired summary immediately in this pending
    /// field. Caller must reserve this enclosing body and whole old/new overlap
    /// before decode/install; local limits alone do not grant native admission.
    pub fn prepare_native_waveform_for_candidate_on_worker(
        &mut self,
        source: &Project,
        candidate: &Project,
        limits: hex_media::waveform::NativeOriginalWaveformLimits,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<(), hex_media::DeviceSourceImportError> {
        self.bound_candidate_asset_on_control(source, candidate)?;
        if self.native_waveform_attempted || self.native_waveform.is_some() {
            return Err(
                ResourcePreparationError::Storage(AudioStorageError::SourceMismatch).into(),
            );
        }
        // Preflight the local complete summary quote before consuming this
        // attempt. No alias/summary birth has occurred at this point.
        let frames = usize::try_from(self.source_frames)
            .map_err(|_| ResourcePreparationError::MetadataCapacity)?;
        if frames > limits.scan_frames
            || hex_media::waveform::native_original_metadata_bytes(frames, limits.bins)?
                > limits.metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity.into());
        }
        self.native_waveform_attempted = true;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &self.source {
                OriginalAudio::Stereo(audio) => {
                    hex_media::waveform::summarize_native_audio_on_worker(audio, limits, cancel)
                }
                OriginalAudio::Mono(source) => {
                    hex_media::waveform::summarize_native_source_on_worker(source, limits, cancel)
                }
            }));
        match result {
            Ok(Ok(waveform)) => {
                // Irreversible fixed field move before any further query,
                // allocation, callback or fallible host operation.
                self.native_waveform = Some(waveform);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(payload) => {
                self.native_waveform_failure = Some(payload);
                Err(hex_media::DeviceSourceImportError::Storage(
                    AudioStorageError::UnknownStorage,
                ))
            }
        }
    }
    /// Borrowed queries only; no owner/source extraction or queued UI access.
    /// ```compile_fail
    /// fn escape(pending: &hex_session::PreparedDeviceSample)
    ///     -> Option<hex_media::waveform::WaveformRead<'static>> {
    ///     pending.native_waveform_read_on_control().unwrap()
    /// }
    /// ```
    /// ```compile_fail
    /// fn extract(pending: &hex_session::PreparedDeviceSample) {
    ///     let _owner = pending.native_waveform.as_ref().unwrap().clone();
    /// }
    /// ```
    pub fn native_waveform_read_on_control(
        &self,
    ) -> Result<Option<hex_media::waveform::WaveformRead<'_>>, AudioStorageError> {
        if self.native_waveform_failure.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(self
            .native_waveform
            .as_ref()
            .map(hex_media::waveform::read_shared))
    }
    /// Only this private field's fixed NativeOriginal constructor can populate
    /// it: paired Waveform + scalar Vecs + native original PCM (no derived
    /// reservation/custom payload). Their concrete control destruction cannot
    /// unwind. A construction panic keeps its first payload and refuses cleanup
    /// or retry, to be retained by the actual failed parent custody.
    pub fn retire_native_waveform_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.native_waveform_failure.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        drop(self.native_waveform.take());
        Ok(())
    }
}
