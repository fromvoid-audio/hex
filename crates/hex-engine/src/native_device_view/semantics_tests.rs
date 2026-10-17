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
