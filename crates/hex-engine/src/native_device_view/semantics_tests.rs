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
        ParameterAutomation::DeviceArrangementEnvelope
    );
    assert!(rate.enabled);
    let state = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    let view = native_device_view(&state).unwrap();
    assert!(view.parameters.iter().all(|parameter| {
        parameter.semantics.is_some_and(|semantics| {
            semantics.access == ParameterAccess::ReadWrite
                && semantics.automation == ParameterAutomation::DeviceArrangementEnvelope
        })
    }));
    let legacy = hex_devices::compressor::COMPRESSOR_DEFINITION
        .default_state()
        .unwrap();
    assert!(
        native_device_view(&legacy)
            .unwrap()
            .parameters
            .iter()
            .all(|parameter| parameter.semantics.is_none())
    );
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

#[test]
fn sampler_replacement_preserves_non_source_state_and_host_envelope_and_refuses_other_modes() {
    use hex_devices::sampler::{
        GateMode, Region,
        state::{AuthoredMode, SamplerState},
    };
    let mut authored = SamplerState::for_source_frames(8).unwrap();
    authored.region = Region { start: 1, end: 7 };
    authored.mode = AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: Region { start: 2, end: 6 },
    };
    authored.root_note = 47;
    authored.reverse = true;
    authored.transpose_semitones = -3.0;
    authored.detune_cents = 7.0;
    authored.gain = 0.4;
    authored.pan = -0.25;
    authored.velocity_amount = 0.5;
    authored.voices = 8;
    authored.envelope.attack_seconds = 0.01;
    authored.envelope.decay_seconds = 0.02;
    authored.envelope.sustain = 0.3;
    authored.envelope.release_seconds = 0.04;
    let old = authored
        .to_native()
        .unwrap()
        .with_input_gain_millidb(3000)
        .unwrap()
        .with_output_gain_millidb(-1000)
        .unwrap();
    let (new, slot) = sampler_replacement_template_on_control(&old, 3).unwrap();
    let mut expected = authored.clone();
    expected.region = Region { start: 0, end: 3 };
    expected.mode = AuthoredMode::Classic {
        loop_enabled: false,
        loop_region: expected.region,
    };
    assert_eq!(SamplerState::from_native(&new).unwrap(), expected);
    assert_eq!(
        (
            new.input_gain_millidb(),
            new.output_gain_millidb(),
            slot.value()
        ),
        (3000, -1000, 1)
    );
    assert!(sampler_replacement_template_on_control(&old, 0).is_err());
    for mode in [
        AuthoredMode::OneShot {
            gate: GateMode::Gate,
        },
        AuthoredMode::Slice {
            starts: vec![1, 4],
            base_note: 60,
            gate: GateMode::Gate,
        },
    ] {
        let mut state = authored.clone();
        state.mode = mode;
        let state = state.to_native().unwrap();
        assert!(validate_sampler_replacement_on_control(&state).is_err());
        assert!(sampler_replacement_template_on_control(&state, 3).is_err());
    }
}

#[test]
fn sampler_facade_delegates_scalar_controls_and_retains_host_errors() {
    use hex_devices::sampler::state::{STATE_VERSION, SamplerState, TYPE_ID};
    let mut authored = SamplerState::for_source_frames(8).unwrap();
    let original = authored.to_native().unwrap();
    let patched =
        patch_sampler_parameter_on_control(SamplerParameter::AttackMs, &original, 125.0).unwrap();
    authored.envelope.attack_seconds = 0.125;
    assert_eq!(patched, authored.to_native().unwrap());
    assert_eq!(
        sampler_parameter_value_on_control(SamplerParameter::AttackMs, &patched).unwrap(),
        125.0
    );
    assert!(sampler_parameter_active_on_control(SamplerParameter::AttackMs, &patched).unwrap());
    for (parameter, value, message) in [
        (
            SamplerParameter::Gain,
            f64::NAN,
            "sampler gain must be between zero and four",
        ),
        (
            SamplerParameter::StartFrame,
            0.5,
            "sampler region endpoints must be bounded whole source frames",
        ),
        (
            SamplerParameter::EndFrame,
            0.0,
            "sampler region endpoints must be bounded whole source frames",
        ),
        (
            SamplerParameter::AttackMs,
            f64::INFINITY,
            "sampler envelope time must be between zero and 60000 ms",
        ),
        (
            SamplerParameter::StartFrame,
            8.0,
            "sampler region must be nonempty and preserve loop and slice bounds",
        ),
    ] {
        assert_eq!(
            patch_sampler_parameter_on_control(parameter, &original, value)
                .unwrap_err()
                .to_string(),
            message
        );
    }
    let unsupported =
        NativeDeviceState::new(TYPE_ID, STATE_VERSION + 1, original.payload().to_vec()).unwrap();
    assert_eq!(
        sampler_parameter_value_on_control(SamplerParameter::Gain, &unsupported)
            .unwrap_err()
            .to_string(),
        "sampler state version is unsupported"
    );
    let other = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    assert_eq!(
        sampler_parameter_active_on_control(SamplerParameter::Gain, &other)
            .unwrap_err()
            .to_string(),
        "sampler state is invalid"
    );
}
