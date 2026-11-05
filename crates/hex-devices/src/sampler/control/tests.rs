use super::*;
use hex_lattice_contract::NativeDeviceMetadata;

#[test]
fn sampler_pitch_patch_preserves_other_authored_fields_and_rejects_invalid_values() {
    use crate::sampler::state::SamplerState;
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let mut authored = SamplerState::for_source_frames(64).unwrap();
    authored.gain = 0.5;
    authored.root_note = 57;
    authored.detune_cents = 23.0;
    let original = authored
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(7).unwrap(),
        }])
        .unwrap()
        .with_input_gain_millidb(2000)
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    for pitch in [-48.0, -12.0, 0.0, 0.5, 12.0, 48.0] {
        let patched =
            patch_sampler_parameter_on_control(SamplerParameter::Transpose, &original, pitch)
                .unwrap();
        assert_eq!(
            sampler_parameter_value_on_control(SamplerParameter::Transpose, &patched).unwrap(),
            pitch
        );
        assert_eq!(
            sampler_parameter_value_on_control(SamplerParameter::Gain, &patched).unwrap(),
            0.5
        );
        assert_eq!(
            patch_sampler_parameter_on_control(SamplerParameter::Transpose, &patched, 0.0).unwrap(),
            original
        );
    }
    for pitch in [-48.01, 48.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            patch_sampler_parameter_on_control(SamplerParameter::Transpose, &original, pitch)
                .is_err()
        );
    }
    let other = crate::utility::UTILITY_DEFINITION.default_state().unwrap();
    assert!(patch_sampler_parameter_on_control(SamplerParameter::Transpose, &other, 0.0).is_err());
}

#[test]
fn sampler_region_patch_is_integral_half_open_and_preserves_loop_slice_and_other_fields() {
    use crate::sampler::{
        GateMode, Region,
        state::{AuthoredMode, SamplerState},
    };
    let mut settings = SamplerState::for_source_frames(8).unwrap();
    settings.gain = 0.5;
    settings.transpose_semitones = 12.0;
    settings.root_note = 57;
    settings.detune_cents = -23.0;
    settings.mode = AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: Region { start: 2, end: 6 },
    };
    let original = settings
        .to_native()
        .unwrap()
        .with_input_gain_millidb(2000)
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    let start =
        patch_sampler_parameter_on_control(SamplerParameter::StartFrame, &original, 1.0).unwrap();
    let end = patch_sampler_parameter_on_control(SamplerParameter::EndFrame, &start, 7.0).unwrap();
    assert_eq!(
        sampler_parameter_value_on_control(SamplerParameter::StartFrame, &end).unwrap(),
        1.0
    );
    assert_eq!(
        sampler_parameter_value_on_control(SamplerParameter::EndFrame, &end).unwrap(),
        7.0
    );
    let reset = patch_sampler_parameter_on_control(SamplerParameter::EndFrame, &end, 8.0).unwrap();
    assert_eq!(
        patch_sampler_parameter_on_control(SamplerParameter::StartFrame, &reset, 0.0).unwrap(),
        original
    );
    for (parameter, value) in [
        (SamplerParameter::StartFrame, 3.0),
        (SamplerParameter::EndFrame, 5.0),
        (SamplerParameter::StartFrame, 8.0),
        (SamplerParameter::EndFrame, 0.0),
        (SamplerParameter::StartFrame, 0.5),
        (SamplerParameter::EndFrame, 7.5),
        (SamplerParameter::EndFrame, f64::NAN),
        (SamplerParameter::EndFrame, f64::INFINITY),
    ] {
        assert!(
            patch_sampler_parameter_on_control(parameter, &original, value).is_err(),
            "{parameter:?} {value}"
        );
    }
    settings.mode = AuthoredMode::Slice {
        starts: vec![0, 2, 5],
        base_note: 60,
        gate: GateMode::Trigger,
    };
    let sliced = settings.to_native().unwrap();
    assert!(
        patch_sampler_parameter_on_control(SamplerParameter::StartFrame, &sliced, 1.0).is_err()
    );
    assert!(patch_sampler_parameter_on_control(SamplerParameter::EndFrame, &sliced, 5.0).is_err());
    let shortened =
        patch_sampler_parameter_on_control(SamplerParameter::EndFrame, &sliced, 6.0).unwrap();
    assert_eq!(
        patch_sampler_parameter_on_control(SamplerParameter::EndFrame, &shortened, 8.0).unwrap(),
        sliced
    );
}

#[test]
fn classic_loop_controls_preserve_dormant_range_refuse_conflicts_and_never_convert_other_modes() {
    use crate::sampler::{
        GateMode,
        state::{AuthoredMode, SamplerState},
    };
    use SamplerParameter::{LoopEnabled, LoopEndFrame, LoopStartFrame, StartFrame};
    let mut authored = SamplerState::for_source_frames(8).unwrap();
    authored.gain = 0.5;
    authored.transpose_semitones = 12.0;
    authored.detune_cents = 23.0;
    let original = authored
        .to_native()
        .unwrap()
        .with_input_gain_millidb(-3000)
        .unwrap();
    let start = patch_sampler_parameter_on_control(LoopStartFrame, &original, 2.0).unwrap();
    let dormant = patch_sampler_parameter_on_control(LoopEndFrame, &start, 6.0).unwrap();
    let on = patch_sampler_parameter_on_control(LoopEnabled, &dormant, 1.0).unwrap();
    assert_eq!(
        sampler_parameter_value_on_control(LoopEnabled, &on).unwrap(),
        1.0
    );
    assert_eq!(
        patch_sampler_parameter_on_control(LoopEnabled, &on, 0.0).unwrap(),
        dormant
    );
    assert_eq!(
        sampler_parameter_value_on_control(LoopStartFrame, &on).unwrap(),
        2.0
    );
    assert_eq!(
        sampler_parameter_value_on_control(LoopEndFrame, &on).unwrap(),
        6.0
    );
    let trimmed = patch_sampler_parameter_on_control(StartFrame, &dormant, 4.0).unwrap();
    assert!(patch_sampler_parameter_on_control(LoopEnabled, &trimmed, 1.0).is_err());
    let repaired = patch_sampler_parameter_on_control(LoopStartFrame, &trimmed, 4.0).unwrap();
    let repaired = patch_sampler_parameter_on_control(LoopEnabled, &repaired, 1.0).unwrap();
    assert!(patch_sampler_parameter_on_control(LoopEndFrame, &repaired, 9.0).is_err());
    assert!(patch_sampler_parameter_on_control(LoopStartFrame, &repaired, 3.0).is_err());
    for value in [0.5, -1.0, 2.0, f64::NAN, f64::INFINITY] {
        assert!(patch_sampler_parameter_on_control(LoopEnabled, &dormant, value).is_err());
    }
    let reset = patch_sampler_parameter_on_control(LoopEnabled, &on, 0.0).unwrap();
    let reset = patch_sampler_parameter_on_control(LoopEndFrame, &reset, 8.0).unwrap();
    assert_eq!(
        patch_sampler_parameter_on_control(LoopStartFrame, &reset, 0.0).unwrap(),
        original
    );
    for mode in [
        AuthoredMode::OneShot {
            gate: GateMode::Gate,
        },
        AuthoredMode::Slice {
            starts: vec![0, 3],
            base_note: 60,
            gate: GateMode::Trigger,
        },
    ] {
        authored.mode = mode;
        let native = authored.to_native().unwrap();
        for parameter in [LoopEnabled, LoopStartFrame, LoopEndFrame] {
            assert!(!sampler_parameter_active_on_control(parameter, &native).unwrap());
            assert!(sampler_parameter_value_on_control(parameter, &native).is_err());
            assert!(patch_sampler_parameter_on_control(parameter, &native, 1.0).is_err());
        }
        assert_eq!(SamplerState::from_native(&native).unwrap(), authored);
    }
}

#[test]
fn sampler_envelope_units_bounds_and_inactive_release_preserve_every_other_field() {
    use crate::sampler::{
        GateMode,
        state::{AuthoredMode, SamplerState},
    };
    use SamplerParameter::{AttackMs, DecayMs, ReleaseMs, SustainPercent};
    for mode in [
        SamplerState::for_source_frames(8).unwrap().mode,
        AuthoredMode::OneShot {
            gate: GateMode::Gate,
        },
        AuthoredMode::OneShot {
            gate: GateMode::Trigger,
        },
        AuthoredMode::Slice {
            starts: vec![0, 3],
            base_note: 60,
            gate: GateMode::Trigger,
        },
    ] {
        let mut authored = SamplerState::for_source_frames(8).unwrap();
        authored.mode = mode.clone();
        authored.gain = 0.5;
        authored.transpose_semitones = -12.0;
        authored.detune_cents = 23.0;
        let original = authored
            .to_native()
            .unwrap()
            .with_input_gain_millidb(-3000)
            .unwrap();
        for parameter in [AttackMs, DecayMs, SustainPercent, ReleaseMs] {
            for value in [0.0, 0.0625, 0.125, parameter.bounds().1] {
                let patched =
                    patch_sampler_parameter_on_control(parameter, &original, value).unwrap();
                let decoded = SamplerState::from_native(&patched).unwrap();
                assert_eq!(
                    sampler_parameter_value_on_control(parameter, &patched).unwrap(),
                    value
                );
                // Independent seconds/fraction oracle: a matching read/write conversion
                // bug must not pass by cancelling itself through the wrapper.
                let mut expected = authored.clone();
                match parameter {
                    AttackMs => expected.envelope.attack_seconds = value * 0.001,
                    DecayMs => expected.envelope.decay_seconds = value * 0.001,
                    SustainPercent => expected.envelope.sustain = value * 0.01,
                    ReleaseMs => expected.envelope.release_seconds = value * 0.001,
                    _ => unreachable!("envelope parameters only"),
                }
                assert_eq!(decoded, expected);
                assert_eq!(patched, expected.replace_in(&original).unwrap());
                assert_eq!(decoded.mode, mode);
                assert_eq!(
                    patch_sampler_parameter_on_control(
                        parameter,
                        &patched,
                        parameter.default_value()
                    )
                    .unwrap(),
                    original
                );
                assert_eq!(
                    sampler_parameter_active_on_control(parameter, &patched).unwrap(),
                    parameter != ReleaseMs
                        || !matches!(
                            mode,
                            AuthoredMode::OneShot {
                                gate: GateMode::Trigger
                            } | AuthoredMode::Slice {
                                gate: GateMode::Trigger,
                                ..
                            }
                        )
                );
            }
            for value in [-0.01, parameter.bounds().1 + 0.01, f64::NAN, f64::INFINITY] {
                assert!(patch_sampler_parameter_on_control(parameter, &original, value).is_err());
            }
        }
    }
}

#[test]
fn strict_current_state_refuses_before_patching_and_preserves_its_source() {
    use crate::sampler::state::{STATE_VERSION, SamplerState, TYPE_ID};
    use hex_lattice_contract::DeviceContractError;
    let original = SamplerState::for_source_frames(16)
        .unwrap()
        .to_native()
        .unwrap();
    let mut nonfinite = original.payload().to_vec();
    nonfinite[40..48].copy_from_slice(&f64::NAN.to_le_bytes());
    for (state, expected) in [
        (
            NativeDeviceState::new(TYPE_ID, STATE_VERSION + 1, original.payload().to_vec())
                .unwrap(),
            DeviceContractError::InvalidStateVersion,
        ),
        (
            crate::utility::UTILITY_DEFINITION.default_state().unwrap(),
            DeviceContractError::WrongDeviceType,
        ),
        (
            NativeDeviceState::new(TYPE_ID, STATE_VERSION, vec![0]).unwrap(),
            DeviceContractError::MalformedState,
        ),
        (
            NativeDeviceState::new(TYPE_ID, original.state_version(), nonfinite).unwrap(),
            DeviceContractError::InvalidParameterValue,
        ),
    ] {
        let before = state.clone();
        assert_eq!(
            sampler_parameter_value_on_control(SamplerParameter::Gain, &state),
            Err(ControlError::State(expected))
        );
        assert_eq!(
            sampler_parameter_active_on_control(SamplerParameter::Gain, &state),
            Err(ControlError::State(expected))
        );
        assert_eq!(
            patch_sampler_parameter_on_control(SamplerParameter::Gain, &state, 0.5),
            Err(ControlError::State(expected))
        );
        assert_eq!(state, before);
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 4.1] {
        assert_eq!(
            patch_sampler_parameter_on_control(SamplerParameter::Gain, &original, value),
            Err(ControlError::InvalidValue(SamplerParameter::Gain))
        );
    }
    let mut expected = SamplerState::from_native(&original).unwrap();
    expected.gain = 0.5;
    assert_eq!(
        patch_sampler_parameter_on_control(SamplerParameter::Gain, &original, 0.5).unwrap(),
        expected.to_native().unwrap()
    );
}

#[test]
fn loop_crossfade_patches_in_classic_mode_only_and_preserves_every_other_field() {
    use crate::sampler::{GateMode, Region, state::AuthoredMode, state::SamplerState};
    use SamplerParameter::LoopCrossfadeMs;
    let mut authored = SamplerState::for_source_frames(64).unwrap();
    authored.mode = AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: Region { start: 8, end: 40 },
    };
    let original = authored
        .to_native()
        .unwrap()
        .with_output_gain_millidb(-2000)
        .unwrap();
    assert_eq!(LoopCrossfadeMs.default_value(), 0.0);
    assert_eq!(LoopCrossfadeMs.unit(), Some("ms"));
    assert_eq!(
        sampler_parameter_value_on_control(LoopCrossfadeMs, &original).unwrap(),
        0.0
    );
    assert!(sampler_parameter_active_on_control(LoopCrossfadeMs, &original).unwrap());
    for value in [0.5, 25.0, LoopCrossfadeMs.bounds().1] {
        let patched =
            patch_sampler_parameter_on_control(LoopCrossfadeMs, &original, value).unwrap();
        assert_eq!(
            sampler_parameter_value_on_control(LoopCrossfadeMs, &patched).unwrap(),
            value
        );
        assert_eq!(patched.output_gain_millidb(), -2000);
        let mut expected = authored.clone();
        expected.loop_crossfade_ms = value;
        assert_eq!(SamplerState::from_native(&patched).unwrap(), expected);
        assert_eq!(
            patch_sampler_parameter_on_control(LoopCrossfadeMs, &patched, 0.0).unwrap(),
            original
        );
    }
    for invalid in [-0.5, f64::NAN, LoopCrossfadeMs.bounds().1 + 1.0] {
        assert_eq!(
            patch_sampler_parameter_on_control(LoopCrossfadeMs, &original, invalid),
            Err(ControlError::InvalidValue(LoopCrossfadeMs))
        );
    }
    authored.mode = AuthoredMode::OneShot {
        gate: GateMode::Gate,
    };
    let one_shot = authored.to_native().unwrap();
    assert!(!sampler_parameter_active_on_control(LoopCrossfadeMs, &one_shot).unwrap());
    assert_eq!(
        sampler_parameter_value_on_control(LoopCrossfadeMs, &one_shot),
        Err(ControlError::RequiresClassic)
    );
    assert_eq!(
        patch_sampler_parameter_on_control(LoopCrossfadeMs, &one_shot, 5.0),
        Err(ControlError::RequiresClassic)
    );
}
