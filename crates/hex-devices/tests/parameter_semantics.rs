use hex_lattice_contract::{
    NativeDeviceMetadata, ParameterAutomation, ParameterNormalization, ParameterValue,
    validated_parameter_semantics,
};
#[test]
fn scalar_auto_pan_declares_implemented_smoothed_automation() {
    use hex_devices::auto_pan::{AUTO_PAN_DEFINITION as d, parameter};
    let state = d.default_state().unwrap();
    for (id, mapping) in [
        (parameter::AMOUNT, ParameterNormalization::Linear),
        (parameter::RATE, ParameterNormalization::PositiveLogarithmic),
        (parameter::RIGHT_PHASE, ParameterNormalization::Linear),
    ] {
        let s = validated_parameter_semantics(&d, &state, id)
            .unwrap()
            .unwrap();
        assert_eq!(s.normalization, mapping);
        assert_eq!(s.automation, ParameterAutomation::DeviceArrangementEnvelope);
        let descriptor = *d.descriptors().iter().find(|v| v.id == id).unwrap();
        let value = d.read_parameter(&state, id).unwrap();
        let normalized = s.normalization.normalize(descriptor, value).unwrap();
        let restored = s.normalization.denormalize(descriptor, normalized).unwrap();
        let (ParameterValue::Float(a), ParameterValue::Float(b)) = (restored, value) else {
            panic!("float expected")
        };
        assert!((a - b).abs() < 1e-4);
    }
}
