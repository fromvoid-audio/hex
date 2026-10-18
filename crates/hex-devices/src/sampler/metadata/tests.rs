use super::*;
use crate::sampler::{PreparedSamplerPlan, Region, SampleChannels, SampleShape};
use hex_lattice::DeviceResourcePcm;
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

fn loaded() -> NativeDeviceState {
    SamplerState::for_source_frames(4)
        .unwrap()
        .to_native()
        .unwrap()
        .with_output_gain_millidb(-600)
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(7).unwrap(),
        }])
        .unwrap()
}

#[test]
fn clean_filter_controls_roundtrip_current_state_host_binding_and_strict_refusal() {
    let original = loaded();
    let mut state = original.clone();
    for (id, value) in [
        (FILTER_ENABLED, ParameterValue::Bool(true)),
        (FILTER_TYPE, ParameterValue::Choice(4)),
        (FILTER_SLOPE, ParameterValue::Choice(0)),
        (FILTER_FREQUENCY, ParameterValue::Float(20_000.0)),
        (FILTER_RESONANCE, ParameterValue::Float(80.0)),
        (FILTER_MORPH, ParameterValue::Float(25.0)),
    ] {
        state = METADATA.patch_parameter(&state, id, value).unwrap();
        assert_eq!(METADATA.read_parameter(&state, id).unwrap(), value);
    }
    let authored = SamplerState::from_native(&state).unwrap();
    let reopened = authored.replace_in(&state).unwrap();
    let read = super::super::state::SamplerStateRead::from_native(&reopened).unwrap();
    assert_eq!(read.filter(), authored.filter);
    assert_eq!(reopened.payload(), state.payload());
    assert_eq!(reopened.asset_references(), original.asset_references());
    assert!(reopened.shares_asset_reference_storage_with(&original));
    assert_eq!(
        reopened.output_gain_millidb(),
        original.output_gain_millidb()
    );
    for mode in [1, 2, 0] {
        state = METADATA
            .patch_parameter(&state, MODE, ParameterValue::Choice(mode))
            .unwrap();
        assert_eq!(
            SamplerState::from_native(&state).unwrap().filter,
            authored.filter
        );
    }
    for (id, value) in [
        (FILTER_FREQUENCY, ParameterValue::Float(f32::NAN)),
        (FILTER_FREQUENCY, ParameterValue::Float(19.0)),
        (FILTER_FREQUENCY, ParameterValue::Float(20_001.0)),
        (FILTER_RESONANCE, ParameterValue::Float(f32::INFINITY)),
        (FILTER_MORPH, ParameterValue::Float(-0.1)),
        (FILTER_CIRCUIT, ParameterValue::Choice(1)),
        (FILTER_TYPE, ParameterValue::Choice(5)),
    ] {
        assert!(METADATA.patch_parameter(&state, id, value).is_err());
    }
    let mut malformed = state.payload().to_vec();
    malformed[112] = 2;
    assert!(
        SamplerState::from_native(
            &NativeDeviceState::new(TYPE_ID, STATE_VERSION, malformed).unwrap()
        )
        .is_err()
    );
    let old = NativeDeviceState::new(TYPE_ID, STATE_VERSION - 1, state.payload().to_vec()).unwrap();
    assert!(METADATA.migrate_state(&old).is_err()); // No historical alpha reader.
}

#[test]
fn one_shot_fades_bind_saved_scalars_to_pcm_and_preserve_host_metadata() {
    let original = loaded();
    assert!(!METADATA.parameter_enabled(&original, FADE_IN).unwrap());
    assert!(
        METADATA
            .patch_parameter(&original, FADE_IN, ParameterValue::Float(0.1))
            .is_err()
    );
    let mode = METADATA
        .patch_parameter(&original, MODE, ParameterValue::Choice(1))
        .unwrap();
    let fade = METADATA
        .patch_parameter(&mode, FADE_IN, ParameterValue::Float(2.0 / 8_000.0))
        .unwrap();
    let fade = METADATA
        .patch_parameter(&fade, FADE_OUT, ParameterValue::Float(2.0 / 8_000.0))
        .unwrap();
    assert_eq!(fade.asset_references(), original.asset_references());
    assert!(fade.shares_asset_reference_storage_with(&original));
    assert_eq!(fade.output_gain_millidb(), -600);
    let authored = SamplerState::from_native(&fade).unwrap();
    let restored = authored.replace_in(&fade).unwrap();
    let read = super::super::state::SamplerStateRead::from_native(&restored).unwrap();
    assert_eq!(
        read.one_shot_fades_seconds(),
        authored.one_shot_fades_seconds
    );
    assert_eq!(
        METADATA.read_parameter(&restored, FADE_IN).unwrap(),
        ParameterValue::Float(2.0 / 8_000.0)
    );
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(4, 8_000, SampleChannels::Stereo).unwrap(),
        8_000,
        authored.settings(),
    )
    .unwrap();
    let coefficients = plan.prepare_voice_coefficients(60, 127).unwrap().unwrap();
    let pcm = [[1.0, -2.0]; 4];
    let mut output = [[99.0; 2]; 4];
    coefficients.render(DeviceResourcePcm::Stereo(&pcm), 0, 0, &mut output);
    assert_eq!(output, [[0.0; 2], [0.5, -1.0], [0.5, -1.0], [0.0; 2]]);
    let classic = METADATA
        .patch_parameter(&restored, MODE, ParameterValue::Choice(0))
        .unwrap();
    let one_shot = METADATA
        .patch_parameter(&classic, MODE, ParameterValue::Choice(1))
        .unwrap();
    assert_eq!(
        SamplerState::from_native(&one_shot)
            .unwrap()
            .one_shot_fades_seconds,
        authored.one_shot_fades_seconds
    );
    for bad in [f32::NAN, f32::INFINITY, -0.1, 60.1] {
        assert!(
            METADATA
                .patch_parameter(&restored, FADE_IN, ParameterValue::Float(bad))
                .is_err()
        );
    }
    let old =
        NativeDeviceState::new(TYPE_ID, STATE_VERSION - 1, restored.payload().to_vec()).unwrap();
    assert!(METADATA.migrate_state(&old).is_err()); // Explicit alpha refusal, no compatibility reader.
}

#[test]
fn source_and_loop_flags_change_audible_frames_without_discarding_markers_or_binding() {
    let original = loaded();
    let region = METADATA
        .patch_parameter(&original, START, ParameterValue::Float(1.0))
        .unwrap();
    let region = METADATA
        .patch_parameter(&region, END, ParameterValue::Float(3.0))
        .unwrap();
    let looped = METADATA
        .patch_parameter(&region, LOOP, ParameterValue::Bool(true))
        .unwrap();
    let looped = METADATA
        .patch_parameter(&looped, LOOP_START, ParameterValue::Float(2.0))
        .unwrap();
    let authored = SamplerState::from_native(&looped).unwrap();
    let coefficients = PreparedSamplerPlan::new(
        SampleShape::new(4, 48_000, SampleChannels::Stereo).unwrap(),
        48_000,
        authored.settings(),
    )
    .unwrap()
    .prepare_voice_coefficients(60, 127)
    .unwrap()
    .unwrap();
    let pcm = [[0.0, 1.0], [0.25, -0.25], [0.5, -0.5], [1.0, 0.0]];
    let mut output = [[0.0; 2]; 5];
    coefficients.render(DeviceResourcePcm::Stereo(&pcm), 0, 16, &mut output);
    assert_eq!(output, [pcm[1], pcm[2], pcm[2], pcm[2], pcm[2]]);
    assert!(looped.shares_asset_reference_storage_with(&original));
    assert_eq!(looped.output_gain_millidb(), -600);
    assert!(
        METADATA
            .patch_parameter(&looped, END, ParameterValue::Float(2.0))
            .is_err()
    );
    assert!(
        METADATA
            .patch_parameter(&looped, LOOP_START, ParameterValue::Float(f32::NAN))
            .is_err()
    );
    assert!(!METADATA.parameter_enabled(&original, LOOP_END).unwrap());
    let mut slice = SamplerState::from_native(&original).unwrap();
    slice.mode = AuthoredMode::Slice {
        starts: vec![0, 2],
        base_note: 60,
        gate: GateMode::Trigger,
    };
    let slice = slice.replace_in(&original).unwrap();
    let moved = METADATA
        .patch_parameter(&slice, START, ParameterValue::Float(1.0))
        .unwrap();
    assert!(
        matches!(SamplerState::from_native(&moved).unwrap().mode, AuthoredMode::Slice { starts, .. } if starts == [1, 2])
    );
    assert!(
        METADATA
            .patch_parameter(&slice, START, ParameterValue::Float(2.0))
            .is_err()
    );
    assert!(
        METADATA
            .patch_parameter(&slice, END, ParameterValue::Float(2.0))
            .is_err()
    );
    assert_eq!(
        METADATA.read_parameter(&slice, START).unwrap(),
        ParameterValue::Float(0.0)
    );
}

#[test]
fn empty_controls_are_disabled_and_malformed_or_nonfinite_changes_refuse() {
    let empty = METADATA.default_state().unwrap();
    assert!(empty.asset_references().is_empty());
    for descriptor in METADATA.descriptors() {
        descriptor.validate_descriptor().unwrap();
        assert!(!METADATA.parameter_enabled(&empty, descriptor.id).unwrap());
        assert_eq!(
            METADATA.read_parameter(&empty, descriptor.id).unwrap(),
            descriptor.default
        );
        assert!(
            METADATA
                .patch_parameter(&empty, descriptor.id, descriptor.default)
                .is_err()
        );
    }
    let state = loaded();
    for value in [f32::NAN, f32::INFINITY, -1.0, 4.1] {
        assert!(
            METADATA
                .patch_parameter(&state, GAIN, ParameterValue::Float(value))
                .is_err()
        );
    }
    assert!(
        METADATA
            .read_parameter(&state, ParameterId::from_static(77))
            .is_err()
    );
    assert!(
        METADATA
            .patch_parameter(&state, MODE, ParameterValue::Choice(3))
            .is_err()
    );
}

#[test]
fn actual_control_edits_change_pcm_and_keep_host_and_source_binding() {
    let original = loaded();
    let edited = METADATA
        .patch_parameter(&original, GAIN, ParameterValue::Float(0.5))
        .unwrap();
    let edited = METADATA
        .patch_parameter(&edited, PAN, ParameterValue::Float(1.0))
        .unwrap();
    let edited = METADATA
        .patch_parameter(&edited, REVERSE, ParameterValue::Bool(true))
        .unwrap();
    assert!(original.shares_asset_reference_storage_with(&edited));
    assert_eq!(edited.output_gain_millidb(), -600);
    let authored = SamplerState::from_native(&edited).unwrap();
    let shape = SampleShape::new(4, 48_000, SampleChannels::Stereo).unwrap();
    let coefficients = PreparedSamplerPlan::new(shape, 48_000, authored.settings())
        .unwrap()
        .prepare_voice_coefficients(60, 127)
        .unwrap()
        .unwrap();
    let pcm = [[1.0, 0.25], [0.5, 0.5], [-0.25, 0.75], [-0.5, 1.0]];
    assert_eq!(
        coefficients.sample(DeviceResourcePcm::Stereo(&pcm), 0, 16),
        [0.0, 0.5]
    );
    assert_eq!(
        coefficients.sample(DeviceResourcePcm::Stereo(&pcm), 3, 16),
        [0.0, 0.125]
    );
    assert_eq!(SamplerState::from_native(&original).unwrap().gain, 1.0);
    let transposed = METADATA
        .patch_parameter(&original, TRANSPOSE, ParameterValue::Float(12.0))
        .unwrap();
    let authored = SamplerState::from_native(&transposed).unwrap();
    let coefficients = PreparedSamplerPlan::new(shape, 48_000, authored.settings())
        .unwrap()
        .prepare_voice_coefficients(60, 127)
        .unwrap()
        .unwrap();
    assert_eq!(
        coefficients.sample(DeviceResourcePcm::Stereo(&pcm), 1, 16),
        pcm[2]
    );
    assert_eq!(coefficients.natural_frames(), Some(2));
}

#[test]
fn mode_specific_controls_and_reselect_preserve_authored_coordinates() {
    let mut authored = SamplerState::from_native(&loaded()).unwrap();
    authored.mode = AuthoredMode::Classic {
        loop_region: Some(Region { start: 1, end: 3 }),
    };
    let state = authored.to_native().unwrap();
    assert_eq!(
        METADATA
            .patch_parameter(&state, MODE, ParameterValue::Choice(0))
            .unwrap(),
        state
    );
    assert_eq!(
        METADATA
            .patch_parameter(&state, LOOP, ParameterValue::Bool(true))
            .unwrap(),
        state
    );
    assert!(!METADATA.parameter_enabled(&state, GATE).unwrap());
    assert!(
        METADATA
            .patch_parameter(&state, GATE, ParameterValue::Choice(1))
            .is_err()
    );
    let one_shot = METADATA
        .patch_parameter(&state, MODE, ParameterValue::Choice(1))
        .unwrap();
    assert!(!METADATA.parameter_enabled(&one_shot, LOOP).unwrap());
    assert!(!METADATA.parameter_enabled(&one_shot, SLICE_BASE).unwrap());
    let gate = METADATA
        .patch_parameter(&one_shot, GATE, ParameterValue::Choice(1))
        .unwrap();
    assert_eq!(
        SamplerState::from_native(&gate).unwrap().mode,
        AuthoredMode::OneShot {
            gate: GateMode::Gate
        }
    );
    let slice = METADATA
        .patch_parameter(&gate, MODE, ParameterValue::Choice(2))
        .unwrap();
    let slice = METADATA
        .patch_parameter(&slice, SLICE_BASE, ParameterValue::Float(72.4))
        .unwrap();
    assert_eq!(
        METADATA.read_parameter(&slice, SLICE_BASE).unwrap(),
        ParameterValue::Float(72.0)
    );
    assert_eq!(
        SamplerState::from_native(&slice).unwrap().mode,
        AuthoredMode::Slice {
            starts: vec![0],
            base_note: 72,
            gate: GateMode::Trigger,
        }
    );
}

#[test]
fn compound_region_flags_cross_old_bounds_in_either_order_and_change_real_pcm() {
    use hex_lattice_contract::DeviceParameterSnapshot;
    let original = loaded();
    let mut state = SamplerState::for_source_frames(8).unwrap();
    state.region = Region { start: 1, end: 3 };
    let original = state.replace_in(&original).unwrap();
    assert!(
        METADATA
            .patch_parameter(&original, START, ParameterValue::Float(5.0))
            .is_err()
    );
    let right = [
        (START, ParameterValue::Float(5.0)),
        (END, ParameterValue::Float(7.0)),
    ];
    let moved = METADATA
        .patch_parameters(&original, &DeviceParameterSnapshot::new(&right).unwrap())
        .unwrap();
    let reversed_order = METADATA
        .patch_parameters(
            &original,
            &DeviceParameterSnapshot::new(&[right[1], right[0]]).unwrap(),
        )
        .unwrap();
    assert_eq!(moved, reversed_order);
    assert!(moved.shares_asset_reference_storage_with(&original));
    assert_eq!(moved.output_gain_millidb(), original.output_gain_millidb());
    assert!(
        METADATA
            .patch_parameter(&moved, END, ParameterValue::Float(3.0))
            .is_err()
    );
    let returned = METADATA
        .patch_parameters(
            &moved,
            &DeviceParameterSnapshot::new(&[
                (END, ParameterValue::Float(3.0)),
                (START, ParameterValue::Float(1.0)),
            ])
            .unwrap(),
        )
        .unwrap();
    assert_eq!(returned, original);
    let pcm = std::array::from_fn::<_, 8, _>(|i| [i as f32 / 8.0, -(i as f32) / 8.0]);
    for (native, expected) in [
        (&original, [pcm[1], pcm[2], [0.0; 2]]),
        (&moved, [pcm[5], pcm[6], [0.0; 2]]),
    ] {
        let authored = SamplerState::from_native(native).unwrap();
        let voice = PreparedSamplerPlan::new(
            SampleShape::new(8, 48_000, SampleChannels::Stereo).unwrap(),
            48_000,
            authored.settings(),
        )
        .unwrap()
        .prepare_voice_coefficients(60, 127)
        .unwrap()
        .unwrap();
        let mut actual = [[0.0; 2]; 3];
        voice.render(DeviceResourcePcm::Stereo(&pcm), 0, 100, &mut actual);
        assert_eq!(actual, expected);
    }
}

#[test]
fn compound_loop_flags_move_past_old_loop_end_and_keep_source_phase_partition() {
    use hex_lattice_contract::DeviceParameterSnapshot;
    let mut state = SamplerState::for_source_frames(8).unwrap();
    state.mode = AuthoredMode::Classic {
        loop_region: Some(Region { start: 1, end: 3 }),
    };
    let original = state.replace_in(&loaded()).unwrap();
    assert!(
        METADATA
            .patch_parameter(&original, LOOP_START, ParameterValue::Float(5.0))
            .is_err()
    );
    let moved = METADATA
        .patch_parameters(
            &original,
            &DeviceParameterSnapshot::new(&[
                (LOOP_START, ParameterValue::Float(5.0)),
                (LOOP_END, ParameterValue::Float(7.0)),
            ])
            .unwrap(),
        )
        .unwrap();
    let authored = SamplerState::from_native(&moved).unwrap();
    let voice = PreparedSamplerPlan::new(
        SampleShape::new(8, 48_000, SampleChannels::Stereo).unwrap(),
        48_000,
        authored.settings(),
    )
    .unwrap()
    .prepare_voice_coefficients(60, 127)
    .unwrap()
    .unwrap();
    let pcm = std::array::from_fn::<_, 8, _>(|i| [i as f32 / 8.0, -(i as f32) / 8.0]);
    let expected = [0, 1, 2, 3, 4, 5, 6, 5, 6, 5, 6].map(|i| pcm[i]);
    let mut whole = [[0.0; 2]; 11];
    voice.render(DeviceResourcePcm::Stereo(&pcm), 0, 100, &mut whole);
    assert_eq!(whole, expected);
    let mut partitioned = [[0.0; 2]; 11];
    for (offset, block) in partitioned.chunks_mut(3).enumerate() {
        voice.render(
            DeviceResourcePcm::Stereo(&pcm),
            (offset * 3) as u64,
            100,
            block,
        );
    }
    assert_eq!(partitioned, expected);
    assert_eq!(SamplerState::from_native(&original).unwrap(), state);
}

#[test]
fn compound_bounds_preserve_slice_markers_and_refuse_invalid_final_geometry_without_mutation() {
    use hex_lattice_contract::DeviceParameterSnapshot;
    let mut state = SamplerState::for_source_frames(8).unwrap();
    state.region.start = 1;
    state.mode = AuthoredMode::Slice {
        starts: vec![1, 5],
        base_note: 60,
        gate: GateMode::Trigger,
    };
    let original = state.replace_in(&loaded()).unwrap();
    let moved = METADATA
        .patch_parameters(
            &original,
            &DeviceParameterSnapshot::new(&[
                (START, ParameterValue::Float(2.0)),
                (END, ParameterValue::Float(7.0)),
            ])
            .unwrap(),
        )
        .unwrap();
    let authored = SamplerState::from_native(&moved).unwrap();
    assert!(matches!(&authored.mode, AuthoredMode::Slice { starts, .. } if starts == &[2, 5]));
    let pcm = std::array::from_fn::<_, 8, _>(|i| [i as f32 / 8.0, -(i as f32) / 8.0]);
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(8, 48_000, SampleChannels::Stereo).unwrap(),
        48_000,
        authored.settings(),
    )
    .unwrap();
    for (note, expected) in [
        (60, [pcm[2], pcm[3], pcm[4], [0.0; 2]]),
        (61, [pcm[5], pcm[6], [0.0; 2], [0.0; 2]]),
    ] {
        let mut actual = [[0.0; 2]; 4];
        plan.prepare_voice_coefficients(note, 127)
            .unwrap()
            .unwrap()
            .render(DeviceResourcePcm::Stereo(&pcm), 0, 100, &mut actual);
        assert_eq!(actual, expected);
    }
    for entries in [
        vec![
            (START, ParameterValue::Float(6.0)),
            (END, ParameterValue::Float(7.0)),
        ],
        vec![
            (START, ParameterValue::Float(2.0)),
            (END, ParameterValue::Float(5.0)),
        ],
        vec![
            (END, ParameterValue::Float(1.0)),
            (START, ParameterValue::Float(2.0)),
        ],
        vec![
            (LOOP_START, ParameterValue::Float(1.0)),
            (LOOP_END, ParameterValue::Float(3.0)),
        ],
        vec![
            (START, ParameterValue::Float(2.0)),
            (GAIN, ParameterValue::Float(4.1)),
        ],
    ] {
        assert!(
            METADATA
                .patch_parameters(&original, &DeviceParameterSnapshot::new(&entries).unwrap())
                .is_err()
        );
        assert_eq!(SamplerState::from_native(&original).unwrap(), state);
        assert_eq!(original.output_gain_millidb(), -600);
    }
    assert!(
        METADATA
            .patch_parameters(
                &METADATA.default_state().unwrap(),
                &DeviceParameterSnapshot::new(&[
                    (START, ParameterValue::Float(1.0)),
                    (END, ParameterValue::Float(3.0))
                ])
                .unwrap()
            )
            .is_err()
    );
}
