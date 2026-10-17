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
struct DynamicDefinition {
    patches: AtomicUsize,
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
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        _id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess as A, ParameterAutomation as E, ParameterChangeImpact as I,
            ParameterModulation as M, ParameterNormalization as N, ParameterSemantics,
            ParameterSmoothing as S, ParameterStepping as T,
        };
        self.migrate_state(state)?;
        if !matches!(state.payload()[0], 8 | 9) {
            return Ok(None);
        }
        Ok(Some(ParameterSemantics {
            access: A::ReadOnly,
            normalization: N::Linear,
            stepping: if state.payload()[0] == 9 {
                T::Choice
            } else {
                T::Continuous
            },
            automation: E::Unsupported,
            modulation: M::Unsupported,
            smoothing: S::None,
            change_impact: I::ValueOnly,
        }))
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
        Ok(state.clone())
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
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
}
impl NativeInstrumentDefinition for DynamicDefinition {
    fn prepare_instrument(
        &self,
        _state: &NativeDeviceState,
        _rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
}
#[test]
fn both_registries_use_current_dynamic_descriptors_for_validation_and_atomic_patches() {
    let definition = DynamicDefinition {
        patches: AtomicUsize::new(0),
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
fn both_registries_refuse_read_only_and_invalid_semantics_before_private_patching() {
    let definition = DynamicDefinition {
        patches: AtomicUsize::new(0),
    };
    let effects: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let instruments: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&effects).unwrap();
    let instruments = NativeInstrumentRegistry::new(&instruments).unwrap();
    let snapshot =
        DeviceParameterSnapshot::new(&[(DYNAMIC_ID, ParameterValue::Float(0.5))]).unwrap();
    for mode in [8, 9] {
        let source = NativeDeviceState::new(definition.type_id(), 1, vec![mode, 0]).unwrap();
        for result in [
            effects.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
            instruments.patch_parameter(&source, DYNAMIC_ID, ParameterValue::Float(0.5)),
            effects.patch_parameters(&source, &snapshot),
            instruments.patch_parameters(&source, &snapshot),
        ] {
            assert!(result.is_err());
        }
        assert_eq!(source.payload(), &[mode, 0]);
    }
    assert_eq!(definition.patches.load(Ordering::Relaxed), 0);
}
