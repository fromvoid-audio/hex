//! Consuming control-side bridge from the existing normalized WAV decoder.
use crate::DecodedAudio;
use hex_engine::{OwnedAudioSource, native_device_resources::ResourcePreparationError};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// Existing native source owner and original path; no second PCM/cache owner.
/// This is source construction, not native host publication or retirement.
pub struct NativeDecodedAudio {
    path: PathBuf,
    source: OwnedAudioSource,
}
impl NativeDecodedAudio {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }
    pub fn frames(&self) -> u64 {
        self.source.frames()
    }
    pub fn into_parts(self) -> (PathBuf, OwnedAudioSource) {
        (self.path, self.source)
    }
}
#[derive(Debug)]
pub struct NativeSourceRefusal {
    pub reason: ResourcePreparationError,
    pub decoded: DecodedAudio,
}
impl DecodedAudio {
    /// Consume ORIGINAL-rate decoded stereo PCM without copying its Vec.
    /// Mono WAV duplication follows the existing decoder contract. Reported
    /// refusals return the original path, metadata and Audio owner intact.
    /// Caller limits bound the scan; construction and all drops stay on control.
    pub fn into_native_source_on_control(
        self,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<NativeDecodedAudio, Box<NativeSourceRefusal>> {
        let Self {
            path,
            audio,
            sample_rate,
            frames,
            file_identity,
        } = self;
        match OwnedAudioSource::try_from_decoded_audio_on_control(audio, max_scan_frames, cancel) {
            Ok(source) => Ok(NativeDecodedAudio { path, source }),
            Err(refused) => Err(Box::new(NativeSourceRefusal {
                reason: refused.reason,
                decoded: Self {
                    path,
                    audio: refused.audio,
                    sample_rate,
                    frames,
                    file_identity,
                },
            })),
        }
    }
}
#[cfg(test)]
mod tests;
