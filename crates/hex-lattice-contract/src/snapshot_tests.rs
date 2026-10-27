use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn snapshot_requires_finite_distinct_bounded_entries() {
    let id = ParameterId::from_static(1);
    assert!(DeviceParameterSnapshot::new(&[]).is_err());
    assert!(DeviceParameterSnapshot::new(&[(id, ParameterValue::Float(f32::NAN))]).is_err());
    assert!(DeviceParameterSnapshot::new(&[(id, ParameterValue::Float(1.0)); 2]).is_err());
    assert!(
        DeviceParameterSnapshot::new(&[(ParameterId::from_static(0), ParameterValue::Bool(true))])
            .is_err()
    );
    let entries: Vec<_> = (1..=65)
        .map(|id| (ParameterId::from_static(id), ParameterValue::Float(0.5)))
        .collect();
    assert!(DeviceParameterSnapshot::new(&entries).is_err());
    let full = DeviceParameterSnapshot::new(&entries[..64]).unwrap();
    assert_eq!(full.entries(), &entries[..64]);
    assert_eq!(std::mem::size_of::<DeviceParameterSnapshot>(), 772);
    assert_eq!(MAX_PARAMETERS_PER_DEVICE, 512);
}

#[test]
fn snapshot_validates_every_descriptor_before_any_patch() {
    let descriptor = ParameterDescriptor {
        id: ParameterId::from_static(1),
        label: "amplitude",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    };
    for value in [
        ParameterValue::Float(-0.1),
        ParameterValue::Float(1.1),
        ParameterValue::Bool(true),
    ] {
        assert!(
            DeviceParameterSnapshot::new(&[(descriptor.id, value)])
                .unwrap()
                .validate(&[descriptor])
                .is_err()
        );
    }
    let a = DeviceParameterSnapshot::new(&[(descriptor.id, ParameterValue::Float(0.2))]).unwrap();
    let b = DeviceParameterSnapshot::new(&[(descriptor.id, ParameterValue::Float(0.8))]).unwrap();
    assert!(a.same_parameters(&b));
    assert!(a.validate(&[descriptor]).is_ok());
    assert!(a.validate(&[]).is_err());
    // Private constructor fills all unused storage identically.
    assert_eq!(a.entries[1..], b.entries[1..]);
}

#[test]
fn snapshot_late_validation_or_private_patch_failure_never_changes_source() {
    struct Definition {
        patches: AtomicUsize,
    }
    const DESCRIPTORS: [ParameterDescriptor; 2] = [
        ParameterDescriptor {
            id: ParameterId::from_static(1),
            label: "first",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(2),
            label: "second",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
    ];
    impl NativeDeviceMetadata for Definition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static("test.snapshot")
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &DESCRIPTORS
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(self.type_id(), 1, vec![0, 0])
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(self.type_id(), 1, state.payload().to_vec())
        }
        fn read_parameter(
            &self,
            _state: &NativeDeviceState,
            _id: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            Ok(ParameterValue::Float(0.0))
        }
        fn patch_parameter(
            &self,
            state: &NativeDeviceState,
            id: ParameterId,
            _value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            assert_eq!(
                state.asset_references().len(),
                1,
                "private patch lost its host binding"
            );
            self.patches.fetch_add(1, Ordering::Relaxed);
            if id.value() == 2 {
                return Err(DeviceContractError::MalformedState);
            }
            let mut bytes = state.payload().to_vec();
            bytes[0] = 1;
            NativeDeviceState::new(self.type_id(), 1, bytes)
        }
    }
    let definition = Definition {
        patches: AtomicUsize::new(0),
    };
    let source = definition
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(42).unwrap(),
        }])
        .unwrap();
    let invalid = DeviceParameterSnapshot::new(&[
        (DESCRIPTORS[0].id, ParameterValue::Float(0.5)),
        (DESCRIPTORS[1].id, ParameterValue::Float(1.5)),
    ])
    .unwrap();
    assert!(definition.patch_parameters(&source, &invalid).is_err());
    assert_eq!(definition.patches.load(Ordering::Relaxed), 0);
    let valid = DeviceParameterSnapshot::new(&[
        (DESCRIPTORS[0].id, ParameterValue::Float(0.5)),
        (DESCRIPTORS[1].id, ParameterValue::Float(0.5)),
    ])
    .unwrap();
    assert!(definition.patch_parameters(&source, &valid).is_err());
    assert_eq!(definition.patches.load(Ordering::Relaxed), 2);
    assert_eq!(source.payload(), &[0, 0]);
}
