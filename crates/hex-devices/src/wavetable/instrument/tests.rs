use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    EMPTY_DEVICE_RESOURCES, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use std::f64::consts::TAU;

fn identity(asset: u64, derivation_version: u32) -> DeviceResourceIdentity {
    DeviceResourceIdentity {
        asset: DeviceAssetKey::new(asset).unwrap(),
        content: [asset as u8; 32],
        derivation_version,
        derivation: [derivation_version as u8; 32],
    }
}

fn ref_for(slot: u32, asset: u64) -> DeviceAssetReference {
    DeviceAssetReference {
        slot: DeviceAssetSlot::new(slot).unwrap(),
        asset: DeviceAssetKey::new(asset).unwrap(),
    }
}

fn two_table_harmonics(frame_size: usize, h0: usize, h1: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frame_size * 2);
    for h in [h0, h1] {
        for i in 0..frame_size {
            let phase = (i as f64) / (frame_size as f64);
            out.push((TAU * (h as f64) * phase).sin() as f32);
        }
    }
    out
}

fn dft_magnitude(signal: &[f32], freq_hz: f64, sample_rate: u32) -> f64 {
    let n = signal.len() as f64;
    let mut re = 0.0_f64;
    let mut im = 0.0_f64;
    for (i, &s) in signal.iter().enumerate() {
        let angle = TAU * freq_hz * (i as f64) / f64::from(sample_rate);
        re += f64::from(s) * angle.cos();
        im -= f64::from(s) * angle.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n
}

#[test]
fn default_state_is_empty_silent_and_refuses_unexpected_resources() {
    let registry = crate::native_instrument_registry().unwrap();
    let def = registry.definition(WAVETABLE_TYPE_ID).unwrap();
    assert!(def.supports_asset_resources());
    assert!(def.resource_factory_inline_bound().is_some_and(|b| b > 0));
    let state = def.default_state().unwrap();
    assert!(state.asset_references().is_empty());

    let tuning = TuningTable::default();
    for rate in [44_100, 48_000, 96_000] {
        let prepared = def
            .prepare_instrument(
                &state,
                &DevicePreparationContext {
                    sample_rate: rate,
                    tuning: &tuning,
                    resources: &EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap();
        let note = prepared.prepare_note(69, 127).unwrap();
        assert_eq!(note.sample(0, 64, &EMPTY_DEVICE_RESOURCES), [0.0; 2]);
        assert_eq!(note.sample(32, 64, &EMPTY_DEVICE_RESOURCES), [0.0; 2]);
    }

    let pcm = two_table_harmonics(256, 1, 3);
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: identity(1, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    assert_eq!(
        def.prepare_instrument(
            &state,
            &DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &BorrowedDeviceResources(&views),
            },
        )
        .err()
        .unwrap(),
        DeviceContractError::AssetResourcesUnavailable
    );
}

#[test]
fn state_codec_parameters_migration_and_malformed_refusals() {
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let authored = WavetableState {
        slot: Some(s1),
        frame_size: 256,
        table_position: 0.35,
        gain: 0.8,
        attack_seconds: 0.005,
        release_seconds: 0.04,
        interpolation: WavetableInterpolation::Linear,
        voice_limit: 8,
    };
    let native = authored
        .to_native_with_asset(ref_for(1, 42))
        .unwrap()
        .with_input_gain_millidb(-150)
        .unwrap()
        .with_output_gain_millidb(200)
        .unwrap();
    assert_eq!(native.payload().len(), PAYLOAD_BYTES_V1);
    assert_eq!(WavetableState::from_native(&native).unwrap(), authored);

    // Parameter read/patch preserves slot, frame_size, asset references, and host gains.
    assert_eq!(
        WAVETABLE_DEFINITION
            .read_parameter(&native, TABLE_POSITION)
            .unwrap(),
        ParameterValue::Float(0.35)
    );
    let patched = WAVETABLE_DEFINITION
        .patch_parameter(&native, TABLE_POSITION, ParameterValue::Float(0.75))
        .unwrap();
    let patched = WAVETABLE_DEFINITION
        .patch_parameter(&patched, INTERPOLATION, ParameterValue::Choice(1))
        .unwrap();
    let patched = WAVETABLE_DEFINITION
        .patch_parameter(&patched, VOICE_LIMIT, ParameterValue::Float(4.0))
        .unwrap();
    assert_eq!(patched.asset_references(), native.asset_references());
    assert_eq!(patched.input_gain_millidb(), -150);
    assert_eq!(patched.output_gain_millidb(), 200);
    let decoded = WavetableState::from_native(&patched).unwrap();
    assert_eq!(decoded.table_position, 0.75);
    assert_eq!(decoded.interpolation, WavetableInterpolation::CubicHermite);
    assert_eq!(decoded.voice_limit, 4);
    assert_eq!(decoded.slot, Some(s1));
    assert_eq!(decoded.frame_size, 256);

    // Migration accepts v1 and rejects v0 / v2+.
    assert_eq!(
        WAVETABLE_DEFINITION.migrate_state(&patched).unwrap(),
        patched
    );
    assert_eq!(
        NativeDeviceState::new(WAVETABLE_TYPE_ID, 0, patched.payload().to_vec()).unwrap_err(),
        DeviceContractError::InvalidStateVersion
    );
    let newer = NativeDeviceState::new(WAVETABLE_TYPE_ID, 2, patched.payload().to_vec())
        .unwrap()
        .with_asset_references(patched.asset_references().to_vec())
        .unwrap();
    assert_eq!(
        WAVETABLE_DEFINITION.migrate_state(&newer).unwrap_err(),
        DeviceContractError::UnsupportedStateVersion
    );

    // Bad frame_size (non-power-of-two), wrong magic, trailing byte, mismatched slot/refs fail.
    let mut bad_fs = authored;
    bad_fs.frame_size = 300;
    assert_eq!(
        bad_fs.validate().unwrap_err(),
        DeviceContractError::MalformedState
    );

    let mut trailing = patched.payload().to_vec();
    trailing.push(0);
    assert_eq!(
        WavetableState::decode_payload(&trailing).unwrap_err(),
        DeviceContractError::MalformedState
    );
    assert_eq!(
        authored.to_native_with_asset(ref_for(2, 42)).unwrap_err(),
        DeviceContractError::MalformedState
    );
    assert_eq!(
        WavetableState::empty()
            .to_native_with_asset(ref_for(1, 42))
            .unwrap_err(),
        DeviceContractError::MalformedState
    );
}

#[test]
fn independent_pitch_table_position_interpolation_spectral_and_partition_oracle() {
    let frame_size = 256_usize;
    let pcm = two_table_harmonics(frame_size, 1, 3);
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let views = [DeviceResourceView {
        slot: s1,
        identity: identity(7, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);

    // Custom tuning: key 69 = 400 Hz (exact integer divisor of 48_000 -> 120 samples/cycle).
    let mut freqs = *TuningTable::default().frequencies();
    freqs[69] = 400.0;
    let tuning = TuningTable::new(freqs).unwrap();
    let rate = 48_000_u32;
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 64,
        max_tail_frames: 48_000 * 600,
    };

    for (pos, expect_h1, expect_h3) in [
        (0.0_f32, 1.0_f64, 0.0_f64),
        (0.5, 0.5, 0.5),
        (1.0, 0.0, 1.0),
    ] {
        for mode in [
            WavetableInterpolation::Linear,
            WavetableInterpolation::CubicHermite,
        ] {
            let state = WavetableState {
                slot: Some(s1),
                frame_size: frame_size as u16,
                table_position: pos,
                gain: 1.0,
                attack_seconds: 0.0,
                release_seconds: 0.005, // 240 frames
                interpolation: mode,
                voice_limit: 8,
            }
            .to_native_with_asset(ref_for(1, 7))
            .unwrap();
            let factory = WAVETABLE_DEFINITION
                .prepare_instrument(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &tuning,
                        resources: &resources,
                    },
                )
                .unwrap()
                .prepare_mutable_runtime()
                .unwrap()
                .unwrap();
            assert_eq!(
                factory.resource_factory_inline_bytes(),
                Some(std::mem::size_of::<PreparedWavetableInstrumentRuntime>())
            );
            let mut whole_pair =
                NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut part_pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();

            let events = [
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 69,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 1_200,
                    occurrence_id: 1,
                },
            ];
            let mut whole = vec![[0.0_f32; 2]; 1_600];
            whole_pair.begin_block();
            whole_pair
                .process_block(&events, &mut whole, &resources)
                .unwrap();
            whole_pair.commit_block();
            assert!(whole_pair.committed_is_quiescent());
            assert!(whole[1_440..].iter().flatten().all(|&s| s == 0.0));

            // Spectral oracle on exact 10-cycle window (1200 frames = 10 * 120 samples at 400 Hz).
            let mono: Vec<f32> = whole[..1_200].iter().map(|f| f[0]).collect();
            let m1 = dft_magnitude(&mono, 400.0, rate);
            let m3 = dft_magnitude(&mono, 1_200.0, rate);
            assert!(
                (m1 - expect_h1).abs() < 0.015 && (m3 - expect_h3).abs() < 0.015,
                "pos={pos} mode={mode:?}: m1={m1} (exp {expect_h1}), m3={m3} (exp {expect_h3})"
            );

            // Partition invariance check.
            let mut parted = vec![[0.0_f32; 2]; 1_600];
            let mut off = 0_usize;
            for len in [1, 17, 128, 300, 754, 400] {
                let local: Vec<_> = events
                    .iter()
                    .filter(|e| (off..off + len).contains(&e.frame_offset()))
                    .map(|&e| match e {
                        NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch,
                            velocity,
                        } => NativeInstrumentEvent::NoteOn {
                            frame_offset: frame_offset - off,
                            occurrence_id,
                            pitch,
                            velocity,
                        },
                        NativeInstrumentEvent::NoteOff {
                            frame_offset,
                            occurrence_id,
                        } => NativeInstrumentEvent::NoteOff {
                            frame_offset: frame_offset - off,
                            occurrence_id,
                        },
                        other => other,
                    })
                    .collect();
                part_pair.begin_block();
                part_pair
                    .process_block(&local, &mut parted[off..off + len], &resources)
                    .unwrap();
                part_pair.commit_block();
                off += len;
            }
            assert_eq!(whole, parted, "partition mismatch pos={pos} mode={mode:?}");
        }
    }
}

#[test]
fn bounded_voices_stealing_held_overflow_and_stale_resource_refusal() {
    let pcm = vec![0.5_f32; 256]; // DC single-cycle wavetable for exact amplitude checks
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let views = [DeviceResourceView {
        slot: s1,
        identity: identity(9, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    let state = WavetableState {
        slot: Some(s1),
        frame_size: 256,
        table_position: 0.0,
        gain: 1.0,
        attack_seconds: 0.0,
        release_seconds: 0.01, // 480 frames
        interpolation: WavetableInterpolation::Linear,
        voice_limit: 1,
    }
    .to_native_with_asset(ref_for(1, 9))
    .unwrap();

    // Missing or unaligned resource refused on control.
    assert_eq!(
        WAVETABLE_DEFINITION
            .prepare_instrument(
                &state,
                &DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &tuning,
                    resources: &EMPTY_DEVICE_RESOURCES,
                },
            )
            .err()
            .unwrap(),
        DeviceContractError::AssetResourcesUnavailable
    );

    let factory = WAVETABLE_DEFINITION
        .prepare_instrument(
            &state,
            &DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &resources,
            },
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 64,
        max_tail_frames: 48_000 * 600,
    };
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
    let mut out = [[0.0_f32; 2]; 16];
    pair.begin_block();
    pair.process_block(
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }],
        &mut out,
        &resources,
    )
    .unwrap();
    pair.commit_block();
    assert!((out[0][0] - 0.5).abs() < 1e-6);

    // Held overflow refuses cleanly without disturbing occurrence 1.
    pair.begin_block();
    assert_eq!(
        pair.process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 64,
                velocity: 127,
            }],
            &mut out,
            &resources,
        )
        .unwrap_err(),
        NativeInstrumentRuntimeError::EventCapacityExceeded
    );

    // Stale derivation_version refuses cleanly with ResourceUnavailable and preserves state.
    let stale_views = [DeviceResourceView {
        slot: s1,
        identity: identity(9, 2),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    pair.begin_block();
    assert_eq!(
        pair.process_block(&[], &mut out, &BorrowedDeviceResources(&stale_views))
            .unwrap_err(),
        NativeInstrumentRuntimeError::ResourceUnavailable
    );

    // Release 1 and trigger 3 -> steals releasing voice 1 with 1 ms declick while 3 plays!
    let mut steal_out = [[0.0_f32; 2]; 64];
    pair.begin_block();
    pair.process_block(
        &[
            NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 3,
                pitch: 64,
                velocity: 127,
            },
        ],
        &mut steal_out,
        &resources,
    )
    .unwrap();
    pair.commit_block();
    assert!(steal_out[4][0] > 0.9); // voice 3 (0.5) + stolen voice 1 declicking (~0.5)
    assert!((steal_out[56][0] - 0.5).abs() < 1e-5); // after 48-frame declick, only voice 3
    let mut active = [0_u64; 4];
    let n = pair.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..n], &[3]);
}
