use super::*;
use hex_lattice_contract::{ParameterAccess, ParameterAutomation, ParameterNormalization};
#[test]
fn cached_view_retains_declarations_and_preserves_undeclared_legacy_controls() {
    let d = &hex_devices::auto_pan::AUTO_PAN_DEFINITION;
    let state = d.default_state().unwrap();
    let view = native_device_view(&state).unwrap();
    let rate = view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == hex_devices::auto_pan::parameter::RATE)
        .unwrap();
    assert_eq!(
        rate.semantics.unwrap().normalization,
        ParameterNormalization::PositiveLogarithmic
    );
    assert_eq!(
        rate.semantics.unwrap().automation,
        ParameterAutomation::Unsupported
    );
    assert!(rate.enabled);
    let state = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    let view = native_device_view(&state).unwrap();
    assert!(view.parameters.iter().all(|p| p.semantics.is_none()));
}
#[test]
fn dynamic_cached_semantics_never_enable_an_ineffective_eq_control() {
    use hex_devices::eq::{EqState, Node, NodeId, NodeKind, NodeParameter};
    let mut settings = EqState::default();
    let id = NodeId::new(8).unwrap();
    let mut node = Node::new(id, 1000.0, 0.0, 0).unwrap();
    node.kind = NodeKind::LowCut;
    settings.add(node).unwrap();
    let view = native_device_view(&settings.encode().unwrap()).unwrap();
    let gain = view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id.parameter(NodeParameter::Gain).unwrap())
        .unwrap();
    assert_eq!(gain.semantics.unwrap().access, ParameterAccess::ReadWrite);
    assert_eq!(
        gain.semantics.unwrap().automation,
        ParameterAutomation::DeviceArrangementEnvelope
    );
    assert!(!gain.enabled);
    settings.remove(id).unwrap();
    assert!(
        native_device_view(&settings.encode().unwrap())
            .unwrap()
            .parameters
            .iter()
            .all(|p| p.descriptor.id != gain.descriptor.id)
    );
}

#[test]
fn sampler_pitch_patch_preserves_other_authored_fields_and_rejects_invalid_values() {
    use hex_devices::sampler::state::SamplerState;
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
    let other = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    assert!(patch_sampler_parameter_on_control(SamplerParameter::Transpose, &other, 0.0).is_err());
}

#[test]
fn sampler_region_patch_is_integral_half_open_and_preserves_loop_slice_and_other_fields() {
    use hex_devices::sampler::{
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
    use SamplerParameter::{LoopEnabled, LoopEndFrame, LoopStartFrame, StartFrame};
    use hex_devices::sampler::{
        GateMode,
        state::{AuthoredMode, SamplerState},
    };
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
    use SamplerParameter::{AttackMs, DecayMs, ReleaseMs, SustainPercent};
    use hex_devices::sampler::{
        GateMode,
        state::{AuthoredMode, SamplerState},
    };
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
fn sampler_history_allows_only_panel_configuration_and_exact_source_host_metadata() {
    use hex_devices::sampler::state::{AuthoredMode, SamplerState};
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let authored = SamplerState::for_source_frames(8).unwrap();
    let current = authored
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(3).unwrap(),
        }])
        .unwrap();
    let mut configured = authored.clone();
    configured.region.start = 1;
    configured.gain = 0.5;
    configured.transpose_semitones = -12.0;
    configured.envelope.attack_seconds = 0.01;
    configured.mode = AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: hex_devices::sampler::Region { start: 1, end: 7 },
    };
    let target = configured.replace_in(&current).unwrap();
    assert!(is_sampler_configuration_history_on_control(&current, &target).unwrap());
    assert!(is_sampler_configuration_history_on_control(&target, &current).unwrap());
    for case in 0..8 {
        let mut unsupported = configured.clone();
        match case {
            0 => unsupported.reverse = true,
            1 => unsupported.root_note = 61,
            2 => unsupported.detune_cents = 1.0,
            3 => unsupported.pan = 0.5,
            4 => unsupported.velocity_amount = 0.5,
            5 => unsupported.voices = 8,
            6 => {
                unsupported.mode = AuthoredMode::OneShot {
                    gate: hex_devices::sampler::GateMode::Gate,
                }
            }
            _ => {
                unsupported.mode = AuthoredMode::Slice {
                    starts: vec![1, 4],
                    base_note: 60,
                    gate: hex_devices::sampler::GateMode::Gate,
                }
            }
        }
        let target = unsupported.replace_in(&current).unwrap();
        assert!(
            !is_sampler_configuration_history_on_control(&current, &target).unwrap(),
            "case{case}"
        );
    }
    for target in [
        target.clone().with_input_gain_millidb(1000).unwrap(),
        target.clone().with_output_gain_millidb(1000).unwrap(),
        target
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(4).unwrap(),
            }])
            .unwrap(),
    ] {
        assert!(!is_sampler_configuration_history_on_control(&current, &target).unwrap());
    }
}
