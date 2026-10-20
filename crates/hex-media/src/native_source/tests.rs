use super::*;
use crate::{Limits, ResampleLimits, decode_wav_file, resample};
use hex_devices::sampler::{PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings};
use hex_engine::{
    AudioStorageReservation, OwnedAudioInputs,
    native_device_resources::{
        DeviceResourceCatalog, OriginalResourceSource, ResourceCatalogReservation,
    },
};
use hex_lattice::{
    DeviceResources, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn decoded(channels: u16) -> (Fixture, DecodedAudio, Vec<[f32; 2]>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-native-ingress-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let expected = if channels == 1 {
        vec![[0.125; 2], [0.375; 2], [0.625; 2], [0.875; 2]]
    } else {
        vec![[0.125, -0.25], [0.375, -0.5], [0.625, -0.75], [0.875, -1.0]]
    };
    let size = 4 * u32::from(channels) * 4;
    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&(36 + size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&3_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&8_000_u32.to_le_bytes());
    wav.extend_from_slice(&(8_000 * u32::from(channels) * 4).to_le_bytes());
    wav.extend_from_slice(&(channels * 4).to_le_bytes());
    wav.extend_from_slice(&32_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&size.to_le_bytes());
    for frame in &expected {
        for sample in &frame[..usize::from(channels)] {
            wav.extend_from_slice(&f32::to_le_bytes(*sample));
        }
    }
    std::fs::write(&path, &wav).unwrap();
    let decoded = decode_wav_file(&path, Limits::new(wav.len(), 4)).unwrap();
    (Fixture(path), decoded, expected)
}
#[test]
fn real_mono_and_stereo_wavs_reach_actual_sampler_from_original_rate_native_bank() {
    for channels in [1, 2] {
        let (file, decoded, expected) = decoded(channels);
        let pointer = decoded.audio().frames().as_ptr();
        let derivative = resample(
            decoded.audio(),
            16_000,
            ResampleLimits {
                max_source_frames: 4,
                max_output_frames: 8,
            },
        )
        .unwrap();
        assert_eq!(derivative.sample_rate(), 16_000);
        assert_eq!(derivative.frames().len(), 8);
        assert_ne!(derivative.frames().as_ptr(), pointer);
        let native = decoded
            .into_native_source_on_control(4, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(native.path(), file.0);
        assert_eq!((native.sample_rate(), native.frames()), (8_000, 4));
        let (path, source) = native.into_parts();
        assert_eq!(path, file.0);
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(16_000).unwrap(), 8).unwrap();
        let hex_project::Created::Imported { asset, .. } = editor
            .execute(hex_project::Command::ImportPreparedAudioTrack {
                name: "native ingress fixture".into(),
                path,
                source_sample_rate: 8_000,
                source_frames: 4,
                playback_frames: 8,
                conversion_version: hex_project::SINC_CONVERSION_V1,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let refused = OwnedAudioInputs::try_new(
            vec![(asset, source)],
            AudioStorageReservation {
                allocation_items: 8,
                handle_obligations: 8,
                max_prepared_data_bytes: 0,
                max_construction_data_bytes: 65_536,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .err()
        .unwrap();
        assert_eq!(
            refused.reason,
            hex_engine::AudioStorageError::ReservationExceeded
        );
        assert_eq!(refused.sources[0].0, asset);
        assert_eq!(refused.sources[0].1.sample_rate(), 8_000);
        assert_eq!(refused.sources[0].1.frames(), 4);
        let bank = OwnedAudioInputs::try_new(
            refused.sources,
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
        let token = bank.source_token(asset).unwrap();
        assert_eq!(
            bank.source_view(&token)
                .unwrap()
                .resident_frames()
                .unwrap()
                .as_ptr(),
            pointer
        );
        let catalog = DeviceResourceCatalog::prepare_on_control(
            &bank,
            &[OriginalResourceSource {
                asset,
                sample_rate: 8_000,
                frames: 4,
            }],
            ResourceCatalogReservation {
                metadata_bytes: 4_096,
                hash_frames: 4,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("test.decoded-sampler"), 1, vec![])
                .unwrap()
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                }])
                .unwrap();
        let scope = catalog.prepare_scope_on_control(&state, 4_096).unwrap();
        let resources = catalog.resources(&bank, &scope).unwrap();
        let prepared = PreparedSamplerPlan::new(
            SampleShape::new(4, 8_000, SampleChannels::Stereo).unwrap(),
            8_000,
            Settings::new(Region { start: 0, end: 4 }),
        )
        .unwrap()
        .bind_source_on_control(resources.get(0).unwrap())
        .unwrap()
        .prepare_runtime_on_control(1, 1, &TuningTable::default())
        .unwrap();
        let mut pair = NativeInstrumentRuntimePair::new(
            &prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 1,
                max_active_occurrences: 1,
                max_tail_frames: 0,
            },
        )
        .unwrap();
        let mut output = [[0.0; 2]; 6];
        pair.begin_block();
        pair.process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }],
            &mut output,
            &resources,
        )
        .unwrap();
        pair.commit_block();
        assert_eq!(&output[..4], expected);
        assert_eq!(&output[4..], [[0.0; 2]; 2]);
        assert!(pair.committed_is_quiescent());
        bank.validate_unresolved_on_control().unwrap();
    }
    eprintln!("real decoded sampler producer: supported-controls=completed");
}
#[test]
fn media_refusals_return_original_decoded_owner_path_and_metadata() {
    let (file, decoded, expected) = decoded(2);
    let pointer = decoded.audio().frames().as_ptr();
    let alias = decoded.clone();
    let refused = decoded
        .into_native_source_on_control(4, &AtomicBool::new(false))
        .err()
        .unwrap();
    assert!(refused.decoded.audio().shares_pcm_with(alias.audio()));
    drop(alias);
    let refused = refused
        .decoded
        .into_native_source_on_control(3, &AtomicBool::new(false))
        .err()
        .unwrap();
    assert_eq!(refused.reason, ResourcePreparationError::WorkCapacity);
    let refused = refused
        .decoded
        .into_native_source_on_control(4, &AtomicBool::new(true))
        .err()
        .unwrap();
    assert_eq!(refused.reason, ResourcePreparationError::Cancelled);
    assert_eq!(refused.decoded.path(), file.0);
    assert_eq!(
        (refused.decoded.sample_rate(), refused.decoded.frames()),
        (8_000, 4)
    );
    assert_eq!(refused.decoded.audio().frames().as_ptr(), pointer);
    assert_eq!(refused.decoded.audio().frames(), expected);
    assert!(
        refused
            .decoded
            .into_native_source_on_control(4, &AtomicBool::new(false))
            .is_ok()
    );
}
