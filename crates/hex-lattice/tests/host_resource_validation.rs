use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    DeviceResources, HostDeviceResourceScope, NoDeviceResourceScope,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceContractError, DeviceTypeId,
    NativeDeviceState,
};
struct Unverified<'a>(BorrowedDeviceResources<'a>);
impl DeviceResources for Unverified<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        self.0.get(index)
    }
}
impl HostDeviceResourceScope for Unverified<'_> {
    fn same_configuration(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
#[test]
fn unverified_scope_default_validation_still_refuses_nonfinite_pcm_and_invalid_facts() {
    let reference = DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(7).unwrap(),
    };
    let state = NativeDeviceState::new(
        DeviceTypeId::from_static("test.unverified-scope"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(vec![reference])
    .unwrap();
    for (samples, source_rate, version, accepted) in [
        (&[0.25, -0.75][..], 44_100, 1, true),
        (&[0.25, f32::NAN][..], 44_100, 1, false),
        (&[0.25, f32::INFINITY][..], 44_100, 1, false),
        (&[][..], 44_100, 1, false),
        (&[0.25][..], 0, 1, false),
        (&[0.25][..], 44_100, 0, false),
    ] {
        let views = [DeviceResourceView {
            slot: reference.slot,
            identity: DeviceResourceIdentity {
                asset: reference.asset,
                content: [1; 32],
                derivation_version: version,
                derivation: [2; 32],
            },
            sample_rate: source_rate,
            pcm: DeviceResourcePcm::Mono(samples),
        }];
        let resources = Unverified(BorrowedDeviceResources(&views));
        assert_eq!(
            resources
                .validate_configuration_on_control(&state, 48_000)
                .is_ok(),
            accepted
        );
        assert_eq!(
            resources.validate_configuration_on_control(&state, 0),
            Err(DeviceContractError::InvalidParameterValue)
        );
    }
    assert_eq!(
        NoDeviceResourceScope.validate_configuration_on_control(&state, 48_000),
        Err(DeviceContractError::AssetResourcesUnavailable)
    );
    assert!(
        NoDeviceResourceScope
            .validate_configuration_on_control(
                &state.with_asset_references(vec![]).unwrap(),
                48_000
            )
            .is_ok()
    );
}
