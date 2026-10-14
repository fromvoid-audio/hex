use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
#[test]
fn published_gain_defaults_validation_rounding_and_processing_identity() {
    let original =
        NativeDeviceState::new(DeviceTypeId::new("vendor.future").unwrap(), 7, vec![0, 255])
            .unwrap();
    assert_eq!(original.output_gain_millidb(), 0);
    assert_eq!(original.output_gain_linear(), 1.0);
    for gain in [-60000, -9500, 24000] {
        let changed = original.clone().with_output_gain_millidb(gain).unwrap();
        assert_eq!(changed.output_gain_millidb(), gain);
        assert!(changed.processing_eq(&original));
        assert_ne!(changed, original);
        assert_eq!(changed.clone().without_output_gain(), original);
        assert_eq!(changed.payload(), original.payload());
    }
    for gain in [-60001, 24001, i32::MIN, i32::MAX] {
        assert_eq!(
            original.clone().with_output_gain_millidb(gain),
            Err(DeviceContractError::InvalidParameterValue)
        );
    }
    for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -60.001, 24.001] {
        assert!(original.clone().with_output_gain_db(gain).is_err());
    }
    assert_eq!(
        original
            .with_output_gain_db(-9.5006)
            .unwrap()
            .output_gain_millidb(),
        -9501
    );
}
