//! Actual original custody; no cache, ownership registry or publication permit.
use crate::{PreparedImport, SessionError};
use hex_engine::{Audio, OwnedAudioSource};
use hex_media::{Limits, ResampleLimits, waveform};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Single worker-owned original. No infallible owning Clone or extra PCM cache.
pub struct DecodedOriginal {
    path: PathBuf,
    source: OriginalAudio,
}
impl DecodedOriginal {
    pub fn decode_file(
        path: impl AsRef<Path>,
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<Self, SessionError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(hex_media::ImportError::Cancelled.into());
        }
        let source = if hex_engine::audio_storage_constructor_layout_supported() {
            Self::native(hex_media::decode_device_source_file(&path, limits, cancel)?)?
        } else {
            OriginalAudio::Stereo(hex_media::decode_wav_file(&path, limits)?.into_audio())
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(hex_media::ImportError::Cancelled.into());
        }
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
    fn native(source: OwnedAudioSource) -> Result<OriginalAudio, SessionError> {
        if source.view().resident_mono_samples().is_some() {
            Ok(OriginalAudio::Mono(source))
        } else {
            source
                .try_into_native_audio_on_control()
                .map(OriginalAudio::Stereo)
                .map_err(|_| SessionError::Invalid("native original source transfer refused"))
        }
    }
    pub(crate) fn from_native(
        path: PathBuf,
        source: OwnedAudioSource,
    ) -> Result<Self, SessionError> {
        Ok(Self {
            path,
            source: Self::native(source)?,
        })
    }
    pub fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }
    pub fn frames(&self) -> usize {
        self.source.frames()
    }
    pub(crate) fn project_audio(
        &self,
        rate: u32,
        limits: ResampleLimits,
    ) -> Result<Audio, SessionError> {
        Ok(match &self.source {
            OriginalAudio::Stereo(audio) => hex_media::resample(audio, rate, limits)?,
            OriginalAudio::Mono(source) => {
                hex_media::resample_mono_original(source.view(), rate, limits)?
            }
        })
    }
    pub fn at_rate(
        self,
        rate: u32,
        limits: ResampleLimits,
    ) -> Result<PreparedImport, SessionError> {
        let audio = self.project_audio(rate, limits)?;
        let waveform = waveform::summarize(&audio, waveform::MAX_BINS)?;
        Ok(PreparedImport {
            source_sample_rate: self.sample_rate(),
            source_frames: self.frames() as u64,
            conversion_version: if self.sample_rate() == rate {
                hex_project::IDENTITY_CONVERSION
            } else {
                hex_project::SINC_CONVERSION_V1
            },
            path: self.path,
            source_audio: self.source,
            audio,
            waveform,
        })
    }
}

pub(crate) enum OriginalAudio {
    Stereo(Audio),
    Mono(OwnedAudioSource),
}
impl OriginalAudio {
    pub(crate) fn stereo(&self) -> Option<&Audio> {
        match self {
            Self::Stereo(audio) => Some(audio),
            Self::Mono(_) => None,
        }
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        match self {
            Self::Stereo(audio) => audio.sample_rate(),
            Self::Mono(source) => source.sample_rate(),
        }
    }
    pub(crate) fn frames(&self) -> usize {
        match self {
            Self::Stereo(audio) => audio.frames().len(),
            Self::Mono(source) => source
                .view()
                .resident_mono_samples()
                .map_or(0, <[f32]>::len),
        }
    }
    pub(crate) fn pcm(&self) -> ResidentPcm<'_> {
        match self {
            Self::Stereo(audio) => ResidentPcm::Stereo(audio),
            Self::Mono(source) => ResidentPcm::Mono(source),
        }
    }
}

/// Borrow actual retained buffers without minting owners or numeric provenance.
#[derive(Clone, Copy)]
pub(crate) enum ResidentPcm<'a> {
    Stereo(&'a Audio),
    Mono(&'a OwnedAudioSource),
}
impl ResidentPcm<'_> {
    pub(crate) fn shares(self, other: Self) -> bool {
        match (self, other) {
            (Self::Stereo(a), Self::Stereo(b)) => a.shares_pcm_with(b),
            (Self::Mono(a), Self::Mono(b)) => match (
                a.view().resident_mono_samples(),
                b.view().resident_mono_samples(),
            ) {
                (Some(a), Some(b)) => std::ptr::eq(a, b),
                _ => false,
            },
            _ => false,
        }
    }
    pub(crate) fn capacity_frames(self) -> Option<usize> {
        match self {
            Self::Stereo(a) => Some(a.pcm_capacity_frames()),
            Self::Mono(a) => a.resident_pcm_capacity_frames(),
        }
    }
}

impl crate::ResidentAsset {
    pub(crate) fn pcm(&self) -> [ResidentPcm<'_>; 2] {
        [self.source_audio.pcm(), ResidentPcm::Stereo(&self.audio)]
    }
}
impl crate::PreparedImport {
    pub(crate) fn pcm(&self) -> [ResidentPcm<'_>; 2] {
        [self.source_audio.pcm(), ResidentPcm::Stereo(&self.audio)]
    }
}
