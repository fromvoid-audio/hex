use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, NativeInstrumentDefinition,
    NativeInstrumentRegistry, PreparedNativeInstrument, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceParameterSnapshot, DeviceTypeId, MAX_PARAMETERS_PER_DEVICE,
    NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind,
    ParameterPresentation, ParameterValue,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const DYNAMIC_ID: ParameterId = ParameterId::from_static(u32::MAX - 2);
const DESCRIPTOR: ParameterDescriptor = ParameterDescriptor {
    id: DYNAMIC_ID,
    label: "dynamic",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 1.0,
        unit: None,
    },
    default: ParameterValue::Float(0.0),
    presentation: ParameterPresentation::Linear,
};
#[derive(Default)]
struct DynamicDefinition {
    patches: AtomicUsize,
    migrations: AtomicUsize,
    preparations: AtomicUsize,
}
impl NativeDeviceMetadata for DynamicDefinition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("test.dynamic")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        Ok(match state.payload()[0] {
            1 => vec![DESCRIPTOR, DESCRIPTOR],
            2 => vec![ParameterDescriptor {
                default: ParameterValue::Float(f32::NAN),
                ..DESCRIPTOR
            }],
            3 => vec![DESCRIPTOR; MAX_PARAMETERS_PER_DEVICE + 1],
            4 => vec![ParameterDescriptor {
                id: ParameterId::from_static(u32::MAX - 1),
                ..DESCRIPTOR
            }],
            5 => vec![ParameterDescriptor {
                label: "Invalid",
                ..DESCRIPTOR
            }],
            6 => vec![ParameterDescriptor {
                id: ParameterId::from_static(0),
                ..DESCRIPTOR
            }],
            7 => vec![],
            _ => vec![DESCRIPTOR],
        })
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.type_id(), 1, vec![0, 0])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != self.type_id()
            || state.state_version() != 1
            || state.payload().len() != 2
        {
            return Err(DeviceContractError::MalformedState);
        }
        self.migrations.fetch_add(1, Ordering::Relaxed);
        NativeDeviceState::new(
            self.type_id(),
            state.state_version(),
            state.payload().to_vec(),
        )
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if parameter != DYNAMIC_ID {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(ParameterValue::Float(f32::from(state.payload()[1]) / 100.0))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        _id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.patches.fetch_add(1, Ordering::Relaxed);
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        NativeDeviceState::new(
            self.type_id(),
            1,
            vec![state.payload()[0], (value * 100.0) as u8],
        )
    }
}
impl NativeDeviceDefinition for DynamicDefinition {
    fn prepare_processor(
        &self,
        _: &NativeDeviceState,
        _: u32,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        Ok(StereoMatrix::identity())
    }
}
impl NativeInstrumentDefinition for DynamicDefinition {
    fn prepare_instrument(
        &self,
        _state: &NativeDeviceState,
        _rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
}
#[test]
fn both_registries_use_current_dynamic_descriptors_for_validation_and_atomic_patches() {
    let definition = DynamicDefinition {
        patches: AtomicUsize::new(0),
        ..Default::default()
    };
    let effects: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let instruments: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let instruments = NativeInstrumentRegistry::new(&instruments).unwrap();
    let source = definition.default_state().unwrap();
    let snapshot =
        DeviceParameterSnapshot::new(&[(DYNAMIC_ID, ParameterValue::Float(0.5))]).unwrap();
    for result in [
        effects.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
        instruments.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
        effects.patch_parameters(&source, &snapshot),
        instruments.patch_parameters(&source, &snapshot),
    ] {
        let patched = result.unwrap();
        assert_eq!(
            definition.read_parameter(&patched, DYNAMIC_ID).unwrap(),
            ParameterValue::Float(0.5)
        );
        assert!(definition.parameter_enabled(&patched, DYNAMIC_ID).unwrap());
    }
    assert_eq!(source.payload(), &[0, 0]);
    let before = definition.patches.load(Ordering::Relaxed);
    let unknown = ParameterId::from_static(u32::MAX - 3);
    let bad_snapshot = DeviceParameterSnapshot::new(&[
        (DYNAMIC_ID, ParameterValue::Float(0.5)),
        (unknown, ParameterValue::Float(0.5)),
    ])
    .unwrap();
    assert!(effects.patch_parameters(&source, &bad_snapshot).is_err());
    assert!(
        instruments
            .patch_parameters(&source, &bad_snapshot)
            .is_err()
    );
    assert!(definition.patch_parameters(&source, &bad_snapshot).is_err());
    assert!(
        effects
            .patch_parameter(&source, unknown, ParameterValue::Float(0.5))
            .is_err()
    );
    assert!(
        instruments
            .patch_parameter(&source, unknown, ParameterValue::Float(0.5))
            .is_err()
    );
    for mode in 1..=7 {
        let state = NativeDeviceState::new(definition.type_id(), 1, vec![mode, 0]).unwrap();
        if mode < 7 {
            assert!(effects.migrate_state(&state).is_err(), "mode {mode}");
            assert!(instruments.migrate_state(&state).is_err(), "mode {mode}");
        }
        assert!(effects.patch_parameters(&state, &snapshot).is_err());
        assert!(instruments.patch_parameters(&state, &snapshot).is_err());
        assert!(definition.parameter_enabled(&state, DYNAMIC_ID).is_err());
        assert!(definition.patch_parameters(&state, &snapshot).is_err());
    }
    assert_eq!(definition.patches.load(Ordering::Relaxed), before);
    let malformed = NativeDeviceState::new(definition.type_id(), 1, vec![]).unwrap();
    assert!(effects.migrate_state(&malformed).is_err());
    assert!(instruments.migrate_state(&malformed).is_err());
}

#[test]
fn device_asset_references_survive_codecs_but_refuse_before_any_preparation() {
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let definition = DynamicDefinition::default();
    let effects: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let instruments: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let instruments = NativeInstrumentRegistry::new(&instruments).unwrap();
    let source = definition
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(3).unwrap(),
            asset: DeviceAssetKey::new(17).unwrap(),
        }])
        .unwrap();
    let snapshot =
        DeviceParameterSnapshot::new(&[(DYNAMIC_ID, ParameterValue::Float(0.5))]).unwrap();
    for result in [
        effects.migrate_state(&source),
        instruments.migrate_state(&source),
        effects.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
        instruments.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
        effects.patch_parameters(&source, &snapshot),
        instruments.patch_parameters(&source, &snapshot),
    ] {
        assert_eq!(
            result.unwrap().asset_references(),
            source.asset_references()
        );
    }
    definition.migrations.store(0, Ordering::Relaxed);
    definition.preparations.store(0, Ordering::Relaxed);
    let refused = DeviceContractError::AssetResourcesUnavailable;
    assert_eq!(effects.prepare_matrix(&source).unwrap_err(), refused);
    assert!(matches!(effects.prepare_processor(&source, 48000), Err(e) if e == refused));
    assert!(matches!(effects.prepare_device(&source, 48000), Err(e) if e == refused));
    assert!(matches!(effects.prepare_runtime_device(&source, 48000), Err(e) if e == refused));
    assert!(matches!(instruments.prepare_instrument(&source, 48000), Err(e) if e == refused));
    assert_eq!(definition.migrations.load(Ordering::Relaxed), 0);
    assert_eq!(definition.preparations.load(Ordering::Relaxed), 0);
    // Calibrate that the fixture observes actual supported preparation calls.
    let empty = source.clone().with_asset_references(vec![]).unwrap();
    effects.prepare_matrix(&empty).unwrap();
    effects.prepare_processor(&empty, 48000).unwrap();
    assert!(instruments.prepare_instrument(&empty, 48000).is_err());
    assert_eq!(definition.preparations.load(Ordering::Relaxed), 3);
}

#[test]
fn manually_prepared_chains_refuse_asset_metadata_including_bypassed_instances() {
    use hex_lattice::{NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner};
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let definition = DynamicDefinition::default();
    let state = definition
        .default_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(42).unwrap(),
        }])
        .unwrap();
    for bypassed in [false, true] {
        let instance = PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 2,
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48000,
            configuration: Some(std::sync::Arc::new(state.clone())),
            definition: None,
        };
        assert!(matches!(
            NativeRuntimeChain::new(vec![instance.clone()]),
            Err(DeviceContractError::AssetResourcesUnavailable)
        ));
        assert!(matches!(
            NativeRuntimeChain::new_prepared(vec![instance.into()]),
            Err(DeviceContractError::AssetResourcesUnavailable)
        ));
    }
}
