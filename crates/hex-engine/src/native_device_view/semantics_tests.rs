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
