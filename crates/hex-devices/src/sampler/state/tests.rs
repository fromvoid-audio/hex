use super::*;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

#[test]
fn current_modes_roundtrip_and_patch_preserves_actual_host_metadata() {
    for mode in [
        AuthoredMode::Classic {
            loop_enabled: false,
            loop_region: Region { start: 3, end: 11 },
        },
        AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: Region { start: 3, end: 11 },
        },
        AuthoredMode::OneShot {
            gate: GateMode::Trigger,
        },
        AuthoredMode::OneShot {
            gate: GateMode::Gate,
        },
        AuthoredMode::Slice {
            starts: vec![0, 3, 11],
            base_note: 60,
            gate: GateMode::Trigger,
        },
    ] {
        let mut state = SamplerState::for_source_frames(16).unwrap();
        state.mode = mode;
        state.reverse = true;
        state.detune_cents = -31.25;
        state.envelope.release_seconds = 0.375;
        state.loop_crossfade_ms = 12.5;
        let native = state
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(SOURCE_SLOT).unwrap(),
                asset: DeviceAssetKey::new(9).unwrap(),
            }])
            .unwrap()
            .with_input_gain_millidb(-1500)
            .unwrap()
            .with_output_gain_millidb(2250)
            .unwrap();
        if native.payload()[0] > 1 {
            let mut reserved = native.payload().to_vec();
            reserved[16] = 1;
            assert_eq!(
                SamplerState::from_native(
                    &NativeDeviceState::new(TYPE_ID, STATE_VERSION, reserved).unwrap()
                ),
                Err(Error::MalformedState)
            );
        }
        assert_eq!(SamplerState::from_native(&native).unwrap(), state);
        state.gain = 0.5;
        let patched = state.replace_in(&native).unwrap();
        assert_eq!(SamplerState::from_native(&patched).unwrap(), state);
        assert!(std::ptr::eq(
            patched.asset_references(),
            native.asset_references()
        ));
        assert_eq!(patched.asset_references(), native.asset_references());
        assert_eq!(patched.input_gain_millidb(), -1500);
        assert_eq!(patched.output_gain_millidb(), 2250);
    }
}

#[test]
fn malformed_truncated_reserved_nonfinite_and_unsupported_formats_refuse() {
    // The v3 layout; v1/v2 layouts are covered by the historical encoder.
    let valid = SamplerState::for_source_frames(16)
        .unwrap()
        .to_current_native()
        .unwrap();
    for end in 0..valid.payload().len() {
        assert!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, STATE_VERSION, valid.payload()[..end].to_vec())
                    .unwrap()
            )
            .is_err()
        );
    }
    let mut trailing = valid.payload().to_vec();
    trailing.push(0);
    assert!(
        SamplerState::from_native(
            &NativeDeviceState::new(TYPE_ID, STATE_VERSION, trailing).unwrap()
        )
        .is_err()
    );
    for (offset, value) in [
        (0, 4),
        (1, 1),
        (2, 2),
        (3, 128),
        (4, 0),
        (4, 33),
        (5, 1),
        (6, 255),
        (20, 0),
    ] {
        let mut bytes = valid.payload().to_vec();
        bytes[offset] = value;
        assert!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes).unwrap()
            )
            .is_err()
        );
    }
    for index in 0..10 {
        let mut bytes = valid.payload().to_vec();
        bytes[24 + index * 8..32 + index * 8].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes).unwrap()
            )
            .is_err()
        );
    }
    // v1 never stored loop coordinates while a classic loop was off.
    let mut v1_off = historical_payload(&SamplerState::for_source_frames(16).unwrap());
    v1_off[16..24].copy_from_slice(&[3, 0, 0, 0, 11, 0, 0, 0]);
    assert_eq!(
        SamplerState::from_native(&NativeDeviceState::new(TYPE_ID, 1, v1_off).unwrap()),
        Err(Error::MalformedState)
    );
    // Each version has exactly its own prefix length.
    for version in [1, 2] {
        assert_eq!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, version, valid.payload().to_vec()).unwrap()
            ),
            Err(Error::MalformedState)
        );
    }
    assert_eq!(
        SamplerState::from_native(
            &NativeDeviceState::new(TYPE_ID, STATE_VERSION, valid.payload()[..96].to_vec())
                .unwrap()
        ),
        Err(Error::MalformedState)
    );
    for crossfade in [-1.0, MAX_LOOP_CROSSFADE_MS + 1.0] {
        let mut bytes = valid.payload().to_vec();
        bytes[96..104].copy_from_slice(&f64::to_le_bytes(crossfade));
        assert_eq!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes).unwrap()
            ),
            Err(Error::InvalidParameterValue)
        );
    }
    assert_eq!(
        SamplerState::from_native(
            &NativeDeviceState::new(TYPE_ID, STATE_VERSION + 1, valid.payload().to_vec()).unwrap()
        ),
        Err(Error::InvalidStateVersion)
    );
    assert_eq!(
        SamplerState::from_native(
            &NativeDeviceState::new(
                DeviceTypeId::from_static("test.other"),
                STATE_VERSION,
                valid.payload().to_vec()
            )
            .unwrap()
        ),
        Err(Error::WrongDeviceType)
    );
    assert!(SamplerState::for_source_frames(0).is_err());
    assert!(SamplerState::for_source_frames(MAX_SAMPLE_FRAMES + 1).is_err());
    let wrong_slot = valid
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(2).unwrap(),
            asset: DeviceAssetKey::new(1).unwrap(),
        }])
        .unwrap();
    assert!(SamplerState::from_native(&wrong_slot).is_err());
}

#[test]
fn decoded_slice_reverse_pan_and_velocity_drive_runtime_after_state_drop() {
    use hex_lattice::{
        BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
        NativeInstrumentEvent, PreparedMutableInstrumentRuntime,
    };
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let mut authored = SamplerState::for_source_frames(6).unwrap();
    authored.mode = AuthoredMode::Slice {
        starts: vec![0, 2, 5],
        base_note: 60,
        gate: GateMode::Trigger,
    };
    authored.reverse = true;
    authored.pan = 1.0;
    authored.velocity_amount = 0.0;
    let native = authored.to_native().unwrap();
    let decoded = SamplerState::from_native(&native).unwrap();
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(6, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        decoded.settings(),
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(
        usize::from(decoded.voices),
        259,
        &hex_lattice::TuningTable::default(),
    )
    .unwrap();
    drop(decoded);
    drop(authored);
    drop(native);
    let mut runtime = prepared.create_runtime();
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 61,
            velocity: 1,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        },
    ];
    let resources = BorrowedDeviceResources(&views);
    let mut output = [[0.0; 2]; 5];
    runtime
        .preflight_events(&events, output.len(), &resources)
        .unwrap();
    runtime.process_block(&events, &mut output, &resources);
    assert_eq!(
        output,
        [[0.0, 9.0], [0.0, 7.0], [0.0, 5.0], [0.0; 2], [0.0; 2]]
    );
    assert!(runtime.is_quiescent());
}

#[test]
fn authored_preparation_requires_exact_asset_and_complete_host_inventory() {
    use hex_lattice::{
        BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity,
        DeviceResourcePcm, DeviceResourceView, NativeInstrumentEvent,
        PreparedMutableInstrumentRuntime, TuningTable,
    };
    let pcm = [1.0, 3.0, 5.0, 7.0];
    let state = SamplerState::for_source_frames(4)
        .unwrap()
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(9).unwrap(),
        }])
        .unwrap();
    let view = DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(9).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    };
    let tuning = TuningTable::default();
    let views = [view];
    let resources = BorrowedDeviceResources(&views);
    let context = DevicePreparationContext {
        sample_rate: 8_000,
        tuning: &tuning,
        resources: &resources,
    };
    let mut dormant = SamplerState::from_native(&state).unwrap();
    dormant.mode = AuthoredMode::Classic {
        loop_enabled: false,
        loop_region: Region { start: 0, end: 5 },
    };
    let dormant = dormant.replace_in(&state).unwrap(); // Global codec bound allows5.
    assert!(
        matches!(
            prepare_runtime_on_control(&dormant, &context, 259),
            Err(Error::InvalidParameterValue)
        ),
        "disabled loop must still fit the actual retained four-frame source"
    );
    let prepared = prepare_runtime_on_control(&state, &context, 259).unwrap();
    let mut runtime = prepared.create_runtime();
    let events = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let mut output = [[0.0; 2]; 5];
    runtime.preflight_events(&events, 5, &resources).unwrap();
    runtime.process_block(&events, &mut output, &resources);
    assert_eq!(output, [[1.0; 2], [3.0; 2], [5.0; 2], [7.0; 2], [0.0; 2]]);
    for variant in 0..6 {
        let mut changed = view;
        match variant {
            0 => changed.slot = DeviceAssetSlot::new(2).unwrap(),
            1 => changed.identity.asset = DeviceAssetKey::new(10).unwrap(),
            2 => changed.pcm = DeviceResourcePcm::Mono(&pcm[..2]),
            3 => changed.pcm = DeviceResourcePcm::Mono(&[1.0, 3.0, 5.0, f32::NAN]),
            4 => changed.sample_rate = 0,
            _ => changed.sample_rate = 192_001,
        }
        let views = [changed];
        let resources = BorrowedDeviceResources(&views);
        let context = DevicePreparationContext {
            sample_rate: 8_000,
            tuning: &tuning,
            resources: &resources,
        };
        assert!(prepare_runtime_on_control(&state, &context, 259).is_err());
    }
    let empty = DevicePreparationContext {
        sample_rate: 8_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    assert!(prepare_runtime_on_control(&state, &empty, 259).is_err());
    let unbound = SamplerState::for_source_frames(4)
        .unwrap()
        .to_native()
        .unwrap();
    assert!(prepare_runtime_on_control(&unbound, &context, 259).is_err());
}

#[test]
fn authored_pitch_gain_region_and_absolute_tuning_reach_actual_pair() {
    use hex_lattice::{
        BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity,
        DeviceResourcePcm, DeviceResourceView, NativeInstrumentEvent,
        NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, TuningTable,
    };
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(9).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let mut authored = SamplerState::for_source_frames(6).unwrap();
    authored.region = Region { start: 1, end: 5 };
    authored.mode = AuthoredMode::OneShot {
        gate: GateMode::Trigger,
    };
    authored.transpose_semitones = 12.0;
    authored.gain = 0.5;
    authored.voices = 1;
    let state = authored
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(9).unwrap(),
        }])
        .unwrap();
    let mut hz = *TuningTable::default().frequencies();
    for frequency in &mut hz {
        *frequency *= 0.5;
    }
    let tuning = TuningTable::new(hz).unwrap();
    let prepared = prepare_runtime_on_control(
        &state,
        &DevicePreparationContext {
            sample_rate: 8_000,
            tuning: &tuning,
            resources: &resources,
        },
        2,
    )
    .unwrap();
    let neutral = prepare_runtime_on_control(
        &state,
        &DevicePreparationContext {
            sample_rate: 8_000,
            tuning: &TuningTable::default(),
            resources: &resources,
        },
        2,
    )
    .unwrap();
    // +12 under neutral tuning is pitched up: the stateless plan query is the
    // band-limited oracle at source positions 0 and 2 of the 4-frame region.
    let octave = crate::sampler::PreparedSamplerPlan::new(
        crate::sampler::SampleShape::new(6, 8_000, crate::sampler::SampleChannels::Mono).unwrap(),
        8_000,
        authored.settings(),
    )
    .unwrap()
    .prepare_tuned_voice_coefficients(60, 127, &TuningTable::default())
    .unwrap()
    .unwrap();
    let neutral_expected: [StereoFrame; 6] =
        std::array::from_fn(|age| octave.sample(DeviceResourcePcm::Mono(&pcm), age as u64, 0));
    assert_eq!(neutral_expected[2..], [[0.0; 2]; 4]);
    assert!(neutral_expected[..2].iter().all(|frame| frame[0] > 0.0));
    drop(state);
    drop(authored);
    // Independent literal oracles separate transpose from project tuning.
    for (prepared, expected) in [
        (neutral, neutral_expected),
        (
            prepared,
            [[1.5; 2], [2.5; 2], [3.5; 2], [4.5; 2], [0.0; 2], [0.0; 2]],
        ),
    ] {
        let mut pair = NativeInstrumentRuntimePair::new(
            &prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 2,
                max_active_occurrences: 1,
                max_tail_frames: 8_000,
            },
        )
        .unwrap();
        let mut output = [[0.0; 2]; 6];
        pair.begin_block();
        pair.process_block(
            &[
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                },
            ],
            &mut output,
            &resources,
        )
        .unwrap();
        pair.commit_block();
        assert_eq!(output, expected);
        assert!(pair.committed_is_quiescent());
    }
}

#[test]
fn disabled_classic_loop_remembers_range_across_codec_and_trim_without_implicit_repair() {
    let mut state = SamplerState::for_source_frames(8).unwrap();
    state.mode = AuthoredMode::Classic {
        loop_enabled: false,
        loop_region: Region { start: 2, end: 6 },
    };
    let original = state.to_native().unwrap();
    // Crossfade off saves the older-reader v2 layout.
    assert_eq!(original.state_version(), 2);
    assert_eq!(original.payload().len(), 96);
    let mut enabled = SamplerState::from_native(&original).unwrap();
    if let AuthoredMode::Classic { loop_enabled, .. } = &mut enabled.mode {
        *loop_enabled = true;
    }
    let enabled = enabled.to_native().unwrap();
    assert_ne!(enabled, original);
    let mut disabled = SamplerState::from_native(&enabled).unwrap();
    if let AuthoredMode::Classic { loop_enabled, .. } = &mut disabled.mode {
        *loop_enabled = false;
    }
    assert_eq!(disabled.to_native().unwrap(), original);
    disabled.region.start = 4;
    assert!(
        disabled.to_native().is_ok(),
        "off loop may lie outside trimmed playback region"
    );
    if let AuthoredMode::Classic { loop_enabled, .. } = &mut disabled.mode {
        *loop_enabled = true;
    }
    assert!(
        disabled.to_native().is_err(),
        "enable must refuse conflicting stored range"
    );
    if let AuthoredMode::Classic { loop_region, .. } = &mut disabled.mode {
        loop_region.start = 4;
    }
    assert!(disabled.to_native().is_ok());
    if let AuthoredMode::Classic {
        loop_enabled,
        loop_region,
    } = &mut disabled.mode
    {
        *loop_enabled = false;
        loop_region.end = loop_region.start;
    }
    assert!(
        disabled.to_native().is_err(),
        "off is not permission for empty loop ranges"
    );
}

/// Historical states keep opening (Pulse #3956). In v1 (commit 57f122b59) a
/// classic loop region existed only while looping; off stored (0, 0).
#[test]
fn sampler_v1_states_migrate_keeping_loop_behaviour_gains_and_source() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let source = vec![DeviceAssetReference {
        slot: DeviceAssetSlot::new(SOURCE_SLOT).unwrap(),
        asset: DeviceAssetKey::new(9).unwrap(),
    }];
    let authored = |loop_enabled| {
        let mut state = SamplerState::for_source_frames(16).unwrap();
        state.region = Region { start: 2, end: 14 };
        state.mode = AuthoredMode::Classic {
            loop_enabled,
            loop_region: Region { start: 3, end: 11 },
        };
        state
    };
    let v1 = |loop_enabled: bool| {
        let mut payload = historical_payload(&authored(loop_enabled));
        if !loop_enabled {
            payload[16..24].fill(0);
        }
        NativeDeviceState::new(TYPE_ID, 1, payload)
            .unwrap()
            .with_asset_references(source.clone())
            .unwrap()
            .with_output_gain_millidb(-1500)
            .unwrap()
    };
    let definition = &super::super::catalog::DEFINITION;
    for loop_enabled in [true, false] {
        let old = v1(loop_enabled);
        let migrated = definition.migrate_state(&old).unwrap();
        assert_eq!(migrated.state_version(), STATE_VERSION);
        assert_eq!(migrated.asset_references(), old.asset_references());
        assert_eq!(migrated.output_gain_millidb(), -1500);
        let mut expected = authored(loop_enabled);
        if !loop_enabled {
            // Off stays off; the dormant loop covers the playback region.
            expected.mode = AuthoredMode::Classic {
                loop_enabled: false,
                loop_region: Region { start: 2, end: 14 },
            };
        }
        assert_eq!(SamplerState::from_native(&migrated).unwrap(), expected);
        assert_eq!(
            definition.migrate_state(&migrated).unwrap(),
            migrated,
            "idempotent"
        );
    }
}

/// Independent v1/v2 encoder (96-byte prefix, no crossfade) for historical
/// fixtures: mode, gate, reverse, root, voices, slice base, slice count, four
/// u32 frames (region, loop region) and nine f64 values, then slice starts.
/// It follows the layout of commit 57f122b59 (v1) and the 21 Oct 2026 v2.
fn historical_payload(state: &SamplerState) -> Vec<u8> {
    let (mode, gate, loop_region, starts, base) = match &state.mode {
        AuthoredMode::Classic {
            loop_enabled,
            loop_region,
        } => (u8::from(*loop_enabled), 0, *loop_region, &[][..], 0),
        AuthoredMode::OneShot { gate } => (
            2,
            u8::from(*gate == GateMode::Trigger),
            Region { start: 0, end: 0 },
            &[][..],
            0,
        ),
        AuthoredMode::Slice {
            starts,
            base_note,
            gate,
        } => (
            3,
            u8::from(*gate == GateMode::Trigger),
            Region { start: 0, end: 0 },
            starts.as_slice(),
            *base_note,
        ),
    };
    let mut bytes = vec![
        mode,
        gate,
        u8::from(state.reverse),
        state.root_note,
        state.voices,
        base,
    ];
    bytes.extend_from_slice(&(starts.len() as u16).to_le_bytes());
    for value in [
        state.region.start,
        state.region.end,
        loop_region.start,
        loop_region.end,
    ] {
        bytes.extend_from_slice(&(value as u32).to_le_bytes());
    }
    for value in [
        state.transpose_semitones,
        state.detune_cents,
        state.gain,
        state.pan,
        state.velocity_amount,
        state.envelope.attack_seconds,
        state.envelope.decay_seconds,
        state.envelope.sustain,
        state.envelope.release_seconds,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(bytes.len(), 96);
    for &start in starts {
        bytes.extend_from_slice(&(start as u32).to_le_bytes());
    }
    bytes
}

/// v2 (21 Oct 2026) states keep opening with the crossfade off (Pulse #4088):
/// every mode, host gains and the source binding survive the registry's v3
/// migration; the saved form stays byte-identical v2 until a crossfade is
/// authored, which saves and reopens as v3; the v2 empty marker stays empty.
#[test]
fn sampler_v2_states_migrate_and_save_as_v2_until_a_crossfade_needs_v3() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let definition = &super::super::catalog::DEFINITION;
    let source = vec![DeviceAssetReference {
        slot: DeviceAssetSlot::new(SOURCE_SLOT).unwrap(),
        asset: DeviceAssetKey::new(9).unwrap(),
    }];
    for mode in [
        AuthoredMode::Classic {
            loop_enabled: false,
            loop_region: Region { start: 3, end: 11 },
        },
        AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: Region { start: 3, end: 11 },
        },
        AuthoredMode::OneShot {
            gate: GateMode::Trigger,
        },
        AuthoredMode::Slice {
            starts: vec![2, 5, 9],
            base_note: 48,
            gate: GateMode::Gate,
        },
    ] {
        let mut expected = SamplerState::for_source_frames(16).unwrap();
        expected.region = Region { start: 2, end: 14 };
        expected.mode = mode;
        expected.reverse = true;
        expected.gain = 0.75;
        expected.envelope.attack_seconds = 0.125;
        let v2 = NativeDeviceState::new(TYPE_ID, 2, historical_payload(&expected))
            .unwrap()
            .with_asset_references(source.clone())
            .unwrap()
            .with_input_gain_millidb(-750)
            .unwrap()
            .with_output_gain_millidb(1250)
            .unwrap();
        // Decoding v2 directly already yields the crossfade-off settings.
        assert_eq!(SamplerState::from_native(&v2).unwrap(), expected);
        let migrated = definition.migrate_state(&v2).unwrap();
        assert_eq!(migrated.state_version(), STATE_VERSION);
        assert_eq!(migrated.payload().len(), v2.payload().len() + 8);
        assert_eq!(migrated.asset_references(), v2.asset_references());
        assert_eq!(migrated.input_gain_millidb(), -750);
        assert_eq!(migrated.output_gain_millidb(), 1250);
        assert_eq!(SamplerState::from_native(&migrated).unwrap(), expected);
        assert_eq!(definition.migrate_state(&migrated).unwrap(), migrated);
        // The saved form of either is the original v2 state, byte for byte,
        // and so is a crossfade-off edit that changes nothing.
        assert_eq!(saved_form(&v2).unwrap(), v2);
        assert_eq!(saved_form(&migrated).unwrap(), v2);
        assert_eq!(expected.replace_in(&v2).unwrap(), v2);
        // An authored crossfade saves as v3 and reopens with the same state.
        let mut edited = expected.clone();
        edited.loop_crossfade_ms = 250.0;
        let saved = edited.replace_in(&v2).unwrap();
        assert_eq!(saved.state_version(), STATE_VERSION);
        assert_eq!(saved.payload().len(), v2.payload().len() + 8);
        assert_eq!(saved_form(&saved).unwrap(), saved);
        let reopened = NativeDeviceState::new(TYPE_ID, STATE_VERSION, saved.payload().to_vec())
            .unwrap()
            .with_host_metadata_from(&saved)
            .unwrap();
        assert_eq!(SamplerState::from_native(&reopened).unwrap(), edited);
    }
    let empty = NativeDeviceState::new(TYPE_ID, 2, b"empty".to_vec())
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    assert!(is_empty_state(&empty));
    let migrated = definition.migrate_state(&empty).unwrap();
    assert_eq!(migrated.state_version(), STATE_VERSION);
    assert!(is_empty_state(&migrated));
    assert_eq!(migrated.output_gain_millidb(), -3000);
    assert_eq!(definition.migrate_state(&migrated).unwrap(), migrated);
    assert_eq!(saved_form(&migrated).unwrap(), empty);
    assert_eq!(
        empty_state().unwrap().state_version(),
        COMPATIBLE_STATE_VERSION
    );
}
