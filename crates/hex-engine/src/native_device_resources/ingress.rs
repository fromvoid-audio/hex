//! Consume an exclusive decoded buffer through the EXISTING native constructor family.
//! The old raw Arc is destroyed on control; its declaration is never upgraded.
use super::*;
use crate::{Audio, AudioData, audio_only_inputs::SourceKind};
use std::sync::Arc;

pub struct ResidentAudioIngressRefusal {
    pub reason: ResourcePreparationError,
    pub audio: Audio,
}
impl crate::OwnedAudioSource {
    /// Bounded, cancellable original-PCM ingress without a duplicate sample buffer.
    /// Shared/weak/derived inputs refuse intact; no caller alias becomes native custody.
    /// A successful move rebuilds the Arc wrapper at the pinned native constructor.
    pub fn try_from_decoded_audio_on_control(
        audio: Audio,
        max_scan_frames: u64,
        cancel: &AtomicBool,
    ) -> Result<Self, ResidentAudioIngressRefusal> {
        Self::decoded_with_gate(
            audio,
            max_scan_frames,
            cancel,
            crate::pcm_arc::native_layout_known(),
        )
    }
    fn decoded_with_gate(
        audio: Audio,
        max_scan_frames: u64,
        cancel: &AtomicBool,
        supported: bool,
    ) -> Result<Self, ResidentAudioIngressRefusal> {
        let checked = (|| {
            cancellation(cancel)?;
            if u64::try_from(audio.frames.values.len()).map_err(|_| AudioStorageError::Overflow)?
                > max_scan_frames
            {
                return Err(ResourcePreparationError::WorkCapacity);
            }
            if audio.sample_rate == 0 || audio.frames.values.is_empty() {
                return Err(ResourcePreparationError::InvalidSource);
            }
            // Counts only refuse outstanding aliases; they never establish provenance.
            if audio.frames._derived.is_some()
                || Arc::strong_count(&audio.frames) != 1
                || Arc::weak_count(&audio.frames) != 0
            {
                return Err(AudioStorageError::SourceMismatch.into());
            }
            for chunk in audio.frames.values.chunks(256) {
                cancellation(cancel)?;
                if chunk.iter().flatten().any(|sample| !sample.is_finite()) {
                    return Err(ResourcePreparationError::InvalidSource);
                }
            }
            if !supported {
                return Err(AudioStorageError::UnsupportedConstructor.into());
            }
            let receipt = crate::pcm_arc::PcmArcReceipt::current(
                audio.frames.values.len(),
                audio.frames.values.capacity(),
            )
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
            let handle = crate::audio_only_inputs::handle()?;
            cancellation(cancel)?;
            Ok((receipt, handle))
        })();
        let (receipt, handle) = match checked {
            Ok(checked) => checked,
            Err(reason) => return Err(ResidentAudioIngressRefusal { reason, audio }),
        };
        let Audio {
            sample_rate,
            frames,
            pcm_arc_receipt,
        } = audio;
        let values = match Arc::try_unwrap(frames) {
            Ok(AudioData {
                values,
                _derived: _,
            }) => values,
            Err(frames) => {
                return Err(ResidentAudioIngressRefusal {
                    reason: AudioStorageError::SourceMismatch.into(),
                    audio: Audio {
                        sample_rate,
                        frames,
                        pcm_arc_receipt,
                    },
                });
            }
        };
        // No fallible operation after consuming the old Arc. This is a fresh
        // constructor-paired allocation around the actual retained Vec, not
        // a receipt attached to an unknown or caller-shared Arc allocation.
        Ok(Self {
            kind: SourceKind::Resident(Audio {
                sample_rate,
                frames: Arc::new(AudioData {
                    values,
                    _derived: None,
                }),
                pcm_arc_receipt: Some(receipt),
            }),
            handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoded_ingress_moves_the_actual_buffer_and_refuses_aliases_without_upgrading_them() {
        let mut frames = Vec::with_capacity(17);
        frames.extend([[0.25, -0.75]; 4]);
        let audio = Audio::new(44_100, frames).unwrap();
        let buffer = audio.frames().as_ptr();
        let alias = audio.clone();
        let refused = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
            audio,
            4,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, AudioStorageError::SourceMismatch.into());
        assert!(Arc::ptr_eq(&refused.audio.frames, &alias.frames));
        assert!(refused.audio.pcm_arc_receipt.is_none());
        drop(alias);
        let weak = Arc::downgrade(&refused.audio.frames);
        let refused = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
            refused.audio,
            4,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, AudioStorageError::SourceMismatch.into());
        assert!(Arc::ptr_eq(&refused.audio.frames, &weak.upgrade().unwrap()));
        drop(weak);
        let refused = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
            refused.audio,
            3,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, ResourcePreparationError::WorkCapacity);
        let refused = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
            refused.audio,
            4,
            &AtomicBool::new(true),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, ResourcePreparationError::Cancelled);
        let result = crate::OwnedAudioSource::try_from_decoded_audio_on_control(
            refused.audio,
            4,
            &AtomicBool::new(false),
        );
        if !crate::pcm_arc::native_layout_known() {
            assert_eq!(
                result.err().unwrap().reason,
                AudioStorageError::UnsupportedConstructor.into()
            );
            return;
        }
        let source = result.ok().unwrap();
        let SourceKind::Resident(audio) = &source.kind else {
            panic!("resident")
        };
        assert_eq!(audio.frames().as_ptr(), buffer);
        assert_eq!(audio.pcm_capacity_frames(), 17);
        assert_eq!(audio.sample_rate(), 44_100);
        assert_eq!(audio.frames(), [[0.25, -0.75]; 4]);
        assert!(
            audio
                .pcm_arc_receipt
                .unwrap()
                .checked_allocations(audio)
                .is_some()
        );
    }
}

#[cfg(test)]
mod unsupported_test {
    use super::*;
    #[test]
    fn unsupported_constructor_returns_original_without_native_provenance() {
        let audio = Audio::new(8_000, vec![[0.25; 2]; 4]).unwrap();
        let pointer = audio.frames().as_ptr();
        let refused =
            crate::OwnedAudioSource::decoded_with_gate(audio, 4, &AtomicBool::new(false), false)
                .err()
                .unwrap();
        assert_eq!(
            refused.reason,
            AudioStorageError::UnsupportedConstructor.into()
        );
        assert_eq!(refused.audio.frames().as_ptr(), pointer);
        assert!(refused.audio.pcm_arc_receipt.is_none());
    }
}
