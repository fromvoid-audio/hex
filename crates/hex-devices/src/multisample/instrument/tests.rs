use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    EMPTY_DEVICE_RESOURCES, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, TuningTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

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

fn oracle_interp_mono(pcm: &[f32], pos: f64) -> [f64; 2] {
    let idx = pos.floor() as usize;
    let frac = pos - pos.floor();
    let a = f64::from(*pcm.get(idx).unwrap_or(&0.0));
    let b = f64::from(*pcm.get(idx + 1).unwrap_or(&0.0));
    let v = a + (b - a) * frac;
    [v, v]
}

fn oracle_interp_stereo(pcm: &[[f32; 2]], pos: f64) -> [f64; 2] {
    let idx = pos.floor() as usize;
    let frac = pos - pos.floor();
    let a = *pcm.get(idx).unwrap_or(&[0.0; 2]);
    let b = *pcm.get(idx + 1).unwrap_or(&[0.0; 2]);
    [
        f64::from(a[0]) + (f64::from(b[0]) - f64::from(a[0])) * frac,
        f64::from(a[1]) + (f64::from(b[1]) - f64::from(a[1])) * frac,
    ]
}

#[test]
fn default_state_is_empty_silent_and_distinct_from_sampler() {
    let registry = crate::native_instrument_registry().unwrap();
    assert_ne!(MULTISAMPLE_TYPE_ID, crate::sampler::state::TYPE_ID);
    let def = registry.definition(MULTISAMPLE_TYPE_ID).unwrap();
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
        let note = prepared.prepare_note(60, 100).unwrap();
        assert_eq!(note.sample(0, 64, &EMPTY_DEVICE_RESOURCES), [0.0; 2]);
        assert_eq!(note.sample(32, 64, &EMPTY_DEVICE_RESOURCES), [0.0; 2]);
    }

    // Empty default state must refuse unexpected host resources.
    let pcm = [0.25_f32; 16];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: identity(1, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let err = def
        .prepare_instrument(
            &state,
            &DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &BorrowedDeviceResources(&views),
            },
        )
        .err()
        .unwrap();
    assert_eq!(err, DeviceContractError::AssetResourcesUnavailable);
}

#[test]
fn state_codec_parameters_migration_and_malformed_refusals() {
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let authored = MultisampleState {
        gain: 0.75,
        overlap_policy: ZoneOverlapPolicy::HighestRootProximity,
        voice_limit: 16,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 36,
                high_key: 60,
                root_key: 48,
                detune_cents: -12.5,
                gain: 0.9,
                release_seconds: 0.05,
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 55,
                high_key: 84,
                root_key: 72,
                detune_cents: 5.0,
                gain: 1.1,
                release_seconds: 0.1,
            },
        ],
    };
    let native = authored
        .to_native_with_assets(vec![ref_for(1, 101), ref_for(2, 202)])
        .unwrap()
        .with_input_gain_millidb(-300)
        .unwrap()
        .with_output_gain_millidb(150)
        .unwrap();
    assert_eq!(native.payload().len(), HEADER_BYTES + 2 * ZONE_BYTES);
    assert_eq!(MultisampleState::from_native(&native).unwrap(), authored);

    // Parameter read/patch preserves zones and host metadata.
    assert_eq!(
        MULTISAMPLE_DEFINITION
            .read_parameter(&native, GAIN)
            .unwrap(),
        ParameterValue::Float(0.75)
    );
    assert_eq!(
        MULTISAMPLE_DEFINITION
            .read_parameter(&native, OVERLAP_POLICY)
            .unwrap(),
        ParameterValue::Choice(1)
    );
    assert_eq!(
        MULTISAMPLE_DEFINITION
            .read_parameter(&native, VOICE_LIMIT)
            .unwrap(),
        ParameterValue::Float(16.0)
    );
    let patched = MULTISAMPLE_DEFINITION
        .patch_parameter(&native, GAIN, ParameterValue::Float(1.25))
        .unwrap();
    let patched = MULTISAMPLE_DEFINITION
        .patch_parameter(&patched, OVERLAP_POLICY, ParameterValue::Choice(2))
        .unwrap();
    let patched = MULTISAMPLE_DEFINITION
        .patch_parameter(&patched, VOICE_LIMIT, ParameterValue::Float(8.0))
        .unwrap();
    assert_eq!(patched.asset_references(), native.asset_references());
    assert_eq!(patched.input_gain_millidb(), -300);
    assert_eq!(patched.output_gain_millidb(), 150);
    let decoded = MultisampleState::from_native(&patched).unwrap();
    assert_eq!(decoded.gain, 1.25);
    assert_eq!(decoded.overlap_policy, ZoneOverlapPolicy::LayerAll);
    assert_eq!(decoded.voice_limit, 8);
    assert_eq!(decoded.zones, authored.zones);

    // Migration accepts v1; v0 is refused by NativeDeviceState::new and v2 by migrate_state.
    assert_eq!(
        MULTISAMPLE_DEFINITION.migrate_state(&patched).unwrap(),
        patched
    );
    assert_eq!(
        NativeDeviceState::new(MULTISAMPLE_TYPE_ID, 0, patched.payload().to_vec()).unwrap_err(),
        DeviceContractError::InvalidStateVersion
    );
    let newer = NativeDeviceState::new(MULTISAMPLE_TYPE_ID, 2, patched.payload().to_vec())
        .unwrap()
        .with_asset_references(patched.asset_references().to_vec())
        .unwrap();
    assert_eq!(
        MULTISAMPLE_DEFINITION.migrate_state(&newer).unwrap_err(),
        DeviceContractError::UnsupportedStateVersion
    );

    // Truncated, trailing, bad magic, non-zero reserved byte, and dangling asset refs fail.
    let mut trailing = patched.payload().to_vec();
    trailing.push(0);
    assert_eq!(
        MultisampleState::decode_payload(&trailing).unwrap_err(),
        DeviceContractError::MalformedState
    );
    assert_eq!(
        MultisampleState::decode_payload(&patched.payload()[..HEADER_BYTES + 4]).unwrap_err(),
        DeviceContractError::MalformedState
    );
    let mut bad_reserved = patched.payload().to_vec();
    bad_reserved[HEADER_BYTES + 7] = 1;
    assert_eq!(
        MultisampleState::decode_payload(&bad_reserved).unwrap_err(),
        DeviceContractError::MalformedState
    );
    // Unbound zone slot refused.
    assert_eq!(
        authored
            .to_native_with_assets(vec![ref_for(1, 101)])
            .unwrap_err(),
        DeviceContractError::MalformedState
    );
    // Unreferenced extra asset slot refused.
    assert_eq!(
        authored
            .to_native_with_assets(vec![ref_for(1, 101), ref_for(2, 202), ref_for(3, 303)])
            .unwrap_err(),
        DeviceContractError::MalformedState
    );
    // Empty zones with dangling asset reference refused.
    assert_eq!(
        MultisampleState::empty()
            .to_native_with_assets(vec![ref_for(1, 101)])
            .unwrap_err(),
        DeviceContractError::MalformedState
    );
}

#[test]
fn independent_source_rate_pitch_key_boundary_and_overlap_pcm_oracle() {
    let pcm_low: Vec<f32> = (0..1024).map(|i| 0.1 + 0.0005 * (i as f32)).collect();
    let pcm_mid: Vec<[f32; 2]> = (0..1024)
        .map(|i| {
            let phase = (i as f32) * 0.05;
            [0.4 * phase.sin(), -0.3 * phase.cos()]
        })
        .collect();
    let pcm_high: Vec<f32> = (0..1024).map(|i| 0.6 - 0.0003 * (i as f32)).collect();

    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let s3 = DeviceAssetSlot::new(3).unwrap();
    let views = [
        DeviceResourceView {
            slot: s1,
            identity: identity(11, 1),
            sample_rate: 44_100,
            pcm: DeviceResourcePcm::Mono(&pcm_low),
        },
        DeviceResourceView {
            slot: s2,
            identity: identity(22, 1),
            sample_rate: 48_000,
            pcm: DeviceResourcePcm::Stereo(&pcm_mid),
        },
        DeviceResourceView {
            slot: s3,
            identity: identity(33, 1),
            sample_rate: 96_000,
            pcm: DeviceResourcePcm::Mono(&pcm_high),
        },
    ];
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    let out_rate = 48_000_u32;

    let zones = vec![
        KeyZoneSpec {
            slot: s1,
            low_key: 36,
            high_key: 60,
            root_key: 48,
            detune_cents: 0.0,
            gain: 0.8,
            release_seconds: 0.001,
        },
        KeyZoneSpec {
            slot: s2,
            low_key: 55,
            high_key: 72,
            root_key: 60,
            detune_cents: 25.0,
            gain: 1.0,
            release_seconds: 0.001,
        },
        KeyZoneSpec {
            slot: s3,
            low_key: 76,
            high_key: 96,
            root_key: 84,
            detune_cents: -15.0,
            gain: 0.7,
            release_seconds: 0.001,
        },
    ];
    let refs = vec![ref_for(1, 11), ref_for(2, 22), ref_for(3, 33)];
    let master_gain = 0.9_f32;

    for policy in [
        ZoneOverlapPolicy::FirstMatch,
        ZoneOverlapPolicy::HighestRootProximity,
        ZoneOverlapPolicy::LayerAll,
    ] {
        let authored = MultisampleState {
            gain: master_gain,
            overlap_policy: policy,
            voice_limit: 16,
            zones: zones.clone(),
        };
        let state = authored.to_native_with_assets(refs.clone()).unwrap();
        let prepared = MULTISAMPLE_DEFINITION
            .prepare_instrument(
                &state,
                &DevicePreparationContext {
                    sample_rate: out_rate,
                    tuning: &tuning,
                    resources: &resources,
                },
            )
            .unwrap();
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        assert_eq!(
            factory.resource_factory_inline_bytes(),
            Some(std::mem::size_of::<PreparedMultisampleInstrumentRuntime>())
        );
        assert!(factory.declared_twin_bytes() * 2 <= 2 * 1024 * 1024);

        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 600,
        };
        let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
        let mut chunked = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();

        // Key 58 sits in overlap of zone 0 (root 48, dist 10) and zone 1 (root 60, dist 2).
        // Key 74 is unmapped -> silence. Key 84 hits zone 2 (96 kHz, -15 ct).
        let events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 58,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 74,
                velocity: 127,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 8,
                occurrence_id: 3,
                pitch: 84,
                velocity: 64,
            },
        ];

        let mut whole = [[0.0_f32; 2]; 64];
        pair.begin_block();
        pair.process_block(&events, &mut whole, &resources).unwrap();
        pair.commit_block();

        // Partition invariance check.
        let mut parts = [[0.0_f32; 2]; 64];
        let mut offset = 0;
        for len in [1, 7, 13, 43] {
            let local: Vec<_> = events
                .iter()
                .filter(|e| (offset..offset + len).contains(&e.frame_offset()))
                .map(|&e| match e {
                    NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    } => NativeInstrumentEvent::NoteOn {
                        frame_offset: frame_offset - offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    },
                    other => other,
                })
                .collect();
            chunked.begin_block();
            chunked
                .process_block(&local, &mut parts[offset..offset + len], &resources)
                .unwrap();
            chunked.commit_block();
            offset += len;
        }
        assert_eq!(whole, parts, "partition invariance failed for {policy:?}");

        // Independent PCM oracle.
        let freqs = tuning.frequencies();
        let step_z0_58 = (freqs[58] / freqs[48]) * (44_100.0 / 48_000.0);
        let step_z1_58 =
            (freqs[58] / freqs[60]) * 2.0_f64.powf(25.0 / 1200.0) * (48_000.0 / 48_000.0);
        let step_z2_84 =
            (freqs[84] / freqs[84]) * 2.0_f64.powf(-15.0 / 1200.0) * (96_000.0 / 48_000.0);

        for (n, actual) in whole.iter().enumerate() {
            let mut expected = [0.0_f64; 2];
            let v1 = 100.0 / 127.0;
            let add_z0 = matches!(
                policy,
                ZoneOverlapPolicy::FirstMatch | ZoneOverlapPolicy::LayerAll
            );
            let add_z1 = matches!(
                policy,
                ZoneOverlapPolicy::HighestRootProximity | ZoneOverlapPolicy::LayerAll
            );
            if add_z0 {
                let s = oracle_interp_mono(&pcm_low, (n as f64) * step_z0_58);
                let sc = 0.8 * v1 * f64::from(master_gain);
                expected[0] += s[0] * sc;
                expected[1] += s[1] * sc;
            }
            if add_z1 {
                let s = oracle_interp_stereo(&pcm_mid, (n as f64) * step_z1_58);
                let sc = 1.0 * v1 * f64::from(master_gain);
                expected[0] += s[0] * sc;
                expected[1] += s[1] * sc;
            }
            if n >= 8 {
                let s = oracle_interp_mono(&pcm_high, ((n - 8) as f64) * step_z2_84);
                let sc = 0.7 * (64.0 / 127.0) * f64::from(master_gain);
                expected[0] += s[0] * sc;
                expected[1] += s[1] * sc;
            }
            for ch in 0..2 {
                assert!(
                    (f64::from(actual[ch]) - expected[ch]).abs() < 1e-5,
                    "policy={policy:?} frame={n} ch={ch} actual={} expected={}",
                    actual[ch],
                    expected[ch]
                );
            }
        }
    }
}

#[test]
fn simultaneous_same_note_note_off_and_voice_stealing_with_declick() {
    let pcm = [0.5_f32; 2048];
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let views = [DeviceResourceView {
        slot: s1,
        identity: identity(1, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();

    // 1. Simultaneous same-note occurrences with independent NoteOff.
    let state_poly = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 4,
        zones: vec![KeyZoneSpec {
            slot: s1,
            low_key: 0,
            high_key: 127,
            root_key: 60,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.0,
        }],
    }
    .to_native_with_assets(vec![ref_for(1, 1)])
    .unwrap();
    let factory = MULTISAMPLE_DEFINITION
        .prepare_instrument(
            &state_poly,
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
    let mut out = [[0.0_f32; 2]; 32];
    pair.begin_block();
    pair.process_block(
        &[
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 2,
                pitch: 60,
                velocity: 127,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 12,
                occurrence_id: 1,
            },
        ],
        &mut out,
        &resources,
    )
    .unwrap();
    pair.commit_block();
    assert!((out[0][0] - 0.5).abs() < 1e-6);
    assert!((out[4][0] - 1.0).abs() < 1e-6); // both simultaneous pitch-60 notes sounding
    assert!((out[12][0] - 0.5).abs() < 1e-6); // only occurrence 1 released; #2 holds
    let mut active = [0_u64; 8];
    let count = pair.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..count], &[2]);

    // 2. Voice limit = 1: stealing oldest releasing voice with 1 ms declick,
    //    while held overflow refuses cleanly without corrupting the held note.
    let pcm_a = [0.4_f32; 4096];
    let pcm_b = [0.2_f32; 4096];
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let views_two = [
        DeviceResourceView {
            slot: s1,
            identity: identity(1, 1),
            sample_rate: 48_000,
            pcm: DeviceResourcePcm::Mono(&pcm_a),
        },
        DeviceResourceView {
            slot: s2,
            identity: identity(2, 1),
            sample_rate: 48_000,
            pcm: DeviceResourcePcm::Mono(&pcm_b),
        },
    ];
    let resources_two = BorrowedDeviceResources(&views_two);
    let state_mono = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 1,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 0,
                high_key: 60,
                root_key: 60,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.01, // 480 frames
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 61,
                high_key: 127,
                root_key: 72,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.01,
            },
        ],
    }
    .to_native_with_assets(vec![ref_for(1, 1), ref_for(2, 2)])
    .unwrap();
    let mono_factory = MULTISAMPLE_DEFINITION
        .prepare_instrument(
            &state_mono,
            &DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &resources_two,
            },
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    assert!(mono_factory.steals_releasing_voices());
    assert_eq!(mono_factory.max_active_occurrences(), 1);

    let mut mono_pair = NativeInstrumentRuntimePair::new(mono_factory.as_ref(), limits).unwrap();
    let mut b1 = [[0.0_f32; 2]; 16];
    mono_pair.begin_block();
    mono_pair
        .process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 10,
                pitch: 60,
                velocity: 127,
            }],
            &mut b1,
            &resources_two,
        )
        .unwrap();
    mono_pair.commit_block();
    assert!((b1[0][0] - 0.4).abs() < 1e-6);

    // Held overflow refuses cleanly and preserves occurrence 10!
    let mut b_refused = [[0.0_f32; 2]; 16];
    mono_pair.begin_block();
    assert_eq!(
        mono_pair
            .process_block(
                &[NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 11,
                    pitch: 72,
                    velocity: 127,
                }],
                &mut b_refused,
                &resources_two,
            )
            .unwrap_err(),
        NativeInstrumentRuntimeError::EventCapacityExceeded
    );
    let count = mono_pair.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..count], &[10]);

    // Release occurrence 10 at offset 0 and retrigger occurrence 12 at offset 4:
    // steals releasing voice 10 into 48-frame (1 ms) declick fade while 12 starts cleanly!
    let mut b_steal = [[0.0_f32; 2]; 64];
    mono_pair.begin_block();
    mono_pair
        .process_block(
            &[
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 10,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 4,
                    occurrence_id: 12,
                    pitch: 72,
                    velocity: 127,
                },
            ],
            &mut b_steal,
            &resources_two,
        )
        .unwrap();
    mono_pair.commit_block();
    // At frame 4: voice 12 = 0.2, stolen voice 10 releasing (rel=4/480) * declick(0/48=1.0).
    let expected_at_4 = 0.2 + 0.4 * (1.0 - 4.0 / 480.0);
    assert!(
        (f64::from(b_steal[4][0]) - expected_at_4).abs() < 1e-5,
        "actual={} expected={expected_at_4}",
        b_steal[4][0]
    );
    // After 48-frame declick finishes (frame 52 = 4 + 48), only voice 12 (0.2) remains!
    assert!((b_steal[52][0] - 0.2).abs() < 1e-6);
    let count = mono_pair.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..count], &[12]);
}

#[test]
fn missing_changed_or_stale_resources_fail_preflight_and_preserve_runtime() {
    let pcm = [0.25_f32; 64];
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let views = [DeviceResourceView {
        slot: s1,
        identity: identity(7, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    let state = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 4,
        zones: vec![KeyZoneSpec {
            slot: s1,
            low_key: 48,
            high_key: 72,
            root_key: 60,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.0,
        }],
    }
    .to_native_with_assets(vec![ref_for(1, 7)])
    .unwrap();

    // Prepare fails with missing or mismatched asset id.
    assert_eq!(
        MULTISAMPLE_DEFINITION
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

    let factory = MULTISAMPLE_DEFINITION
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
    assert!((out[0][0] - 0.25).abs() < 1e-6);

    // Stale derivation_version fails preflight with ResourceUnavailable.
    let stale_views = [DeviceResourceView {
        slot: s1,
        identity: identity(7, 2),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    pair.begin_block();
    assert_eq!(
        pair.process_block(&[], &mut out, &BorrowedDeviceResources(&stale_views))
            .unwrap_err(),
        NativeInstrumentRuntimeError::ResourceUnavailable
    );
    // Original resource still works cleanly on the next block (state preserved).
    pair.begin_block();
    pair.process_block(&[], &mut out, &resources).unwrap();
    pair.commit_block();
    assert!((out[0][0] - 0.25).abs() < 1e-6);
}
