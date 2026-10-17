use super::*;

fn reference(slot: u32, asset: u64) -> DeviceAssetReference {
    DeviceAssetReference {
        slot: DeviceAssetSlot::new(slot).unwrap(),
        asset: DeviceAssetKey::new(asset).unwrap(),
    }
}
fn state(version: u32) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("test.asset"), version, vec![17]).unwrap()
}

#[test]
fn asset_envelope_is_bounded_canonical_and_separate_from_payload() {
    assert!(DeviceAssetSlot::new(0).is_err());
    assert!(DeviceAssetKey::new(0).is_err());
    let source = state(1);
    assert!(source.asset_references.is_none());
    let a = source
        .clone()
        .with_asset_references(vec![reference(9, 42), reference(1, 42)])
        .unwrap();
    let b = source
        .clone()
        .with_asset_references(vec![reference(1, 42), reference(9, 42)])
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(a.payload(), source.payload());
    assert!(!a.processing_eq(&source));
    assert!(
        source
            .clone()
            .with_asset_references(vec![reference(1, 42); 2])
            .is_err()
    );
    let full: Vec<_> = (1..=MAX_DEVICE_ASSET_REFERENCES as u32)
        .map(|slot| reference(slot, 42))
        .collect();
    assert!(source.clone().with_asset_references(full.clone()).is_ok());
    let mut overflow = full;
    overflow.push(reference(999, 42));
    assert!(source.clone().with_asset_references(overflow).is_err());
    assert!(
        a.with_asset_references(vec![])
            .unwrap()
            .asset_references
            .is_none()
    );
    assert!(
        NativeDeviceState::new(
            source.type_id(),
            1,
            vec![0; MAX_NATIVE_DEVICE_STATE_BYTES + 1]
        )
        .is_err()
    );
}

#[test]
fn host_metadata_survives_version_migration_and_affects_processing_identity() {
    let source = state(1)
        .with_asset_references(vec![reference(1, 42)])
        .unwrap()
        .with_input_gain_millidb(2000)
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    let migrated = state(2).with_host_metadata_from(&source).unwrap();
    assert_eq!(migrated.asset_references(), source.asset_references());
    assert_eq!(migrated.input_gain_millidb(), 2000);
    assert_eq!(migrated.output_gain_millidb(), -3000);
    let without_gains = source.clone().without_host_gains();
    assert!(without_gains.processing_eq(&source));
    assert_eq!(without_gains.asset_references(), source.asset_references());
    assert_eq!(
        source.require_asset_free_processing(),
        Err(DeviceContractError::AssetResourcesUnavailable)
    );
    assert!(state(1).require_asset_free_processing().is_ok());
    let other = NativeDeviceState::new(DeviceTypeId::from_static("test.other"), 1, vec![]).unwrap();
    assert_eq!(
        other.with_host_metadata_from(&source),
        Err(DeviceContractError::WrongDeviceType)
    );
}
