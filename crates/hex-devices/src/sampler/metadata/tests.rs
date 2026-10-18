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
