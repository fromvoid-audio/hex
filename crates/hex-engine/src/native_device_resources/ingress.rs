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
            native_handle,
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
                        native_handle,
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
                native_handle: std::num::NonZeroU64::new(handle),
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
mod audio_lineage_tests {
    use super::*;
    use crate::{
        AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageReservation,
        AudioStorageRole, OwnedAudioInputs, OwnedAudioSource,
    };
    #[test]
    fn actual_stereo_owner_moves_and_tracked_audio_clones_join_the_same_native_union() {
        let mut values = Vec::with_capacity(17);
        values.extend([[0.25, -0.75]; 4]);
        let pointer = values.as_ptr();
        let source = match OwnedAudioSource::try_resident(44_100, values) {
            Ok(source) => source,
            Err(refused) => {
                assert_eq!(refused.reason, AudioStorageError::UnsupportedConstructor);
                eprintln!("unsupported native Audio constructor; no lineage proof");
                return;
            }
        };
        let handle = source.handle;
        let audio = source.try_into_native_audio_on_control().ok().unwrap();
        assert_eq!(audio.native_handle.map(|h| h.get()), Some(handle));
        assert_eq!(audio.frames().as_ptr(), pointer);
        let alias = audio.clone();
        let alias_handle = alias.native_handle.unwrap().get();
        assert_ne!(alias_handle, handle);
        assert!(audio.shares_pcm_with(&alias));
        let alias = OwnedAudioSource::try_from_native_audio_on_control(alias)
            .ok()
            .unwrap();
        assert_eq!(alias.handle, alias_handle);
        let asset = super::super::tests::ids()[0];
        let bank = OwnedAudioInputs::try_new(
            vec![(asset, alias)],
            AudioStorageReservation {
                allocation_items: 8,
                handle_obligations: 8,
                max_prepared_data_bytes: 65_536,
                max_construction_data_bytes: 65_536,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap()
        .prepare_input_storage_on_control()
        .ok()
        .unwrap();
        let catalog = DeviceResourceCatalog::prepare_on_control(
            &bank,
            &[OriginalResourceSource {
                asset,
                sample_rate: 44_100,
                frames: 4,
                layout: OriginalResourceLayout::Stereo,
            }],
            ResourceCatalogReservation {
                metadata_bytes: 65_536,
                hash_frames: 4,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let bank = DeviceResourceBank::try_new_on_control(
            bank,
            catalog,
            65_536,
            65_536,
            &AtomicBool::new(false),
        )
        .ok()
        .unwrap();
        let source = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 16,
                root_rows: 8,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)
            .unwrap();
        ledger
            .borrow_original_source_lease_on_control(&source, AudioStorageRole::Archive)
            .unwrap();
        assert!(ledger.validate_closed_shared_handles().is_err());
        ledger
            .borrow_native_audio_on_control(&audio, AudioStorageRole::Caller)
            .unwrap();
        ledger.validate_closed_shared_handles().unwrap();
        assert_eq!(ledger.shared_resource_counts(), (2, 4)); // PCM + root; all real handles
        drop(ledger);
        drop(audio);
        drop(bank);
        assert_eq!(
            source.view().unwrap().resident_frames().unwrap().as_ptr(),
            pointer
        );
        assert_eq!(
            source.view().unwrap().resident_frames().unwrap(),
            [[0.25, -0.75]; 4]
        );
    }
    #[test]
    fn unknown_or_unqualified_audio_cannot_be_relabelled_by_clone_or_native_ingress() {
        let raw = Audio::new(48_000, vec![[0.25; 2]; 4]).unwrap();
        let pointer = raw.frames().as_ptr();
        assert!(raw.native_handle.is_none());
        assert!(raw.clone().native_handle.is_none());
        let refusal = OwnedAudioSource::try_from_native_audio_on_control(raw)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::UnknownStorage);
        assert_eq!(refusal.audio.frames().as_ptr(), pointer);
        let source = match OwnedAudioSource::try_resident(48_000, vec![[0.5; 2]; 4]) {
            Ok(source) => source,
            Err(refused) => {
                assert_eq!(refused.reason, AudioStorageError::UnsupportedConstructor);
                return;
            }
        };
        let native = source.try_into_native_audio_on_control().ok().unwrap();
        // Same path as identity exhaustion: retain immutable PCM, refuse native
        // qualification. Do not mutate the global allocator to fake exhaustion.
        let unqualified = native.clone_with_native_handle_on_control(None);
        assert!(unqualified.shares_pcm_with(&native));
        assert!(unqualified.native_handle.is_none());
        let refusal = OwnedAudioSource::try_from_native_audio_on_control(unqualified)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::UnknownStorage);
        assert!(refusal.audio.shares_pcm_with(&native));
    }
}
