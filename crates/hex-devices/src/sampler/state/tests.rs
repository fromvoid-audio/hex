use super::*;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

#[test]
fn empty_device_is_unbound_and_first_load_has_real_bounds_without_invented_media() {
    let empty = DeviceState::Empty.to_native().unwrap();
    assert!(empty.asset_references().is_empty());
    assert_eq!(
        DeviceState::from_native(&empty).unwrap(),
        DeviceState::Empty
    );
    assert!(SamplerState::from_native(&empty).is_err());
    assert!(replacement_state_on_control(&empty, 0).is_err());
    assert!(replacement_state_on_control(&empty, MAX_SAMPLE_FRAMES + 1).is_err());
    let loaded = replacement_state_on_control(&empty, 7).unwrap();
    assert_eq!(
        SamplerState::from_native(&loaded).unwrap(),
        SamplerState::for_source_frames(7).unwrap()
    );
    assert!(loaded.asset_references().is_empty()); // canonical host binds actual new asset later
    let false_empty = empty
        .clone()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(SOURCE_SLOT).unwrap(),
            asset: DeviceAssetKey::new(9).unwrap(),
        }])
        .unwrap();
    assert!(DeviceState::from_native(&false_empty).is_err());
    assert!(replacement_state_on_control(&false_empty, 7).is_err());
    assert!(
        DeviceState::from_native(
            &NativeDeviceState::new(TYPE_ID, STATE_VERSION + 1, vec![]).unwrap()
        )
        .is_err()
    );
}

#[test]
fn replacement_preserves_sound_and_host_controls_but_resets_old_source_coordinates() {
    for mode in [
        AuthoredMode::Classic {
            loop_region: Some(Region { start: 4, end: 11 }),
        },
        AuthoredMode::OneShot {
            gate: GateMode::Gate,
        },
        AuthoredMode::Slice {
            starts: vec![2, 4, 11],
            base_note: 48,
            gate: GateMode::Trigger,
        },
    ] {
        let mut original = SamplerState::for_source_frames(16).unwrap();
        original.region.start = 2;
        original.mode = mode;
        original.reverse = true;
        original.root_note = 57;
        original.transpose_semitones = 3.5;
        original.detune_cents = -25.0;
        original.gain = 0.375;
        original.pan = -0.5;
        original.velocity_amount = 0.25;
        original.envelope.attack_seconds = 0.125;
        original.envelope.release_seconds = 0.5;
        original.voices = 8;
        let current = original
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
        let replacement = replacement_state_on_control(&current, 3).unwrap();
        let loaded = SamplerState::from_native(&replacement).unwrap();
        let expected_mode = match original.mode {
            AuthoredMode::Classic { .. } => AuthoredMode::Classic { loop_region: None },
            AuthoredMode::OneShot { gate } => AuthoredMode::OneShot { gate },
            AuthoredMode::Slice {
                base_note, gate, ..
            } => AuthoredMode::Slice {
                starts: vec![0],
                base_note,
                gate,
            },
        };
        original.region = Region { start: 0, end: 3 };
        original.mode = expected_mode;
        assert_eq!(loaded, original);
        assert_eq!(replacement.input_gain_millidb(), -1500);
        assert_eq!(replacement.output_gain_millidb(), 2250);
        assert!(replacement.shares_asset_reference_storage_with(&current));
    }
}

#[test]
fn current_modes_roundtrip_and_patch_preserves_actual_host_metadata() {
    for mode in [
        AuthoredMode::Classic { loop_region: None },
        AuthoredMode::Classic {
            loop_region: Some(Region { start: 3, end: 11 }),
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
        assert_eq!(SamplerState::from_native(&native).unwrap(), state);
        state.gain = 0.5;
        let patched = state.replace_in(&native).unwrap();
        assert_eq!(SamplerState::from_native(&patched).unwrap(), state);
        assert_eq!(patched.asset_references(), native.asset_references());
        assert_eq!(patched.input_gain_millidb(), -1500);
        assert_eq!(patched.output_gain_millidb(), 2250);
        assert!(patched.shares_asset_reference_storage_with(&native));
    }
}

#[test]
fn malformed_truncated_reserved_nonfinite_and_unsupported_formats_refuse() {
    let valid = SamplerState::for_source_frames(16)
        .unwrap()
        .to_native()
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
        (20, 1),
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
    for index in 0..9 {
        let mut bytes = valid.payload().to_vec();
        bytes[24 + index * 8..32 + index * 8].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(
            SamplerState::from_native(
                &NativeDeviceState::new(TYPE_ID, STATE_VERSION, bytes).unwrap()
            )
            .is_err()
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
    for variant in 0..4 {
        let mut changed = view;
        match variant {
            0 => changed.slot = DeviceAssetSlot::new(2).unwrap(),
            1 => changed.identity.asset = DeviceAssetKey::new(10).unwrap(),
            2 => changed.pcm = DeviceResourcePcm::Mono(&pcm[..2]),
            _ => changed.pcm = DeviceResourcePcm::Mono(&[1.0, 3.0, 5.0, f32::NAN]),
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
