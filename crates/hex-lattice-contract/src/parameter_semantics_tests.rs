use super::*;
const ID: ParameterId = ParameterId::from_static(1);
const DESCRIPTOR: ParameterDescriptor = ParameterDescriptor {
    id: ID,
    label: "value",
    kind: ParameterKind::Float {
        min: 1.0,
        max: 100.0,
        unit: None,
    },
    default: ParameterValue::Float(10.0),
    presentation: ParameterPresentation::Linear,
};
fn semantics() -> ParameterSemantics {
    ParameterSemantics {
        access: ParameterAccess::ReadWrite,
        normalization: ParameterNormalization::PositiveLogarithmic,
        stepping: ParameterStepping::Continuous,
        automation: ParameterAutomation::Unsupported,
        modulation: ParameterModulation::Unsupported,
        smoothing: ParameterSmoothing::None,
        change_impact: ParameterChangeImpact::ValueOnly,
    }
}
struct Definition {
    declaration: Option<ParameterSemantics>,
}
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("fixture.semantics")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[DESCRIPTOR]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.type_id(), 1, vec![])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != self.type_id() {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != 1 {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.payload().is_empty() {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if id != ID {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(DESCRIPTOR.default)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        _id: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(state.clone())
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        self.migrate_state(state)?;
        if id != ID {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(self.declaration)
    }
}
#[test]
fn undeclared_membership_invalid_declarations_and_read_only_atomic_guard() {
    let mut definition = Definition { declaration: None };
    let state = definition.default_state().unwrap();
    assert_eq!(
        validated_parameter_semantics(&definition, &state, ID).unwrap(),
        None
    );
    assert_eq!(
        validated_parameter_semantics(&definition, &state, ParameterId::from_static(2)),
        Err(DeviceContractError::UnknownParameter)
    );
    let mut invalid = semantics();
    invalid.stepping = ParameterStepping::Choice;
    definition.declaration = Some(invalid);
    assert_eq!(
        validated_state_parameters(&definition, &state),
        Err(DeviceContractError::InvalidDescriptor)
    );
    let mut read_only = semantics();
    read_only.access = ParameterAccess::ReadOnly;
    definition.declaration = Some(read_only);
    let snapshot = DeviceParameterSnapshot::new(&[(ID, DESCRIPTOR.default)]).unwrap();
    assert_eq!(
        definition.patch_parameters(&state, &snapshot),
        Err(DeviceContractError::InvalidParameterValue)
    );
    read_only.automation = ParameterAutomation::DeviceArrangementEnvelope;
    assert_eq!(
        read_only.validate(DESCRIPTOR),
        Err(DeviceContractError::InvalidDescriptor)
    );
    let malformed = NativeDeviceState::new(definition.type_id(), 1, vec![1]).unwrap();
    assert_eq!(
        validated_parameter_semantics(&definition, &malformed, ID),
        Err(DeviceContractError::MalformedState)
    );
}
#[test]
fn normalized_numeric_edges_discrete_midpoints_and_singletons_are_explicit() {
    let log = ParameterNormalization::PositiveLogarithmic;
    assert_eq!(
        log.normalize(DESCRIPTOR, ParameterValue::Float(1.0))
            .unwrap(),
        0.0
    );
    assert_eq!(
        log.normalize(DESCRIPTOR, ParameterValue::Float(100.0))
            .unwrap(),
        1.0
    );
    assert_eq!(
        log.denormalize(DESCRIPTOR, 0.5).unwrap(),
        ParameterValue::Float(10.0)
    );
    for n in [0.0, 0.1, 0.25, 0.5, 0.9, 1.0] {
        let value = log.denormalize(DESCRIPTOR, n).unwrap();
        assert!((log.normalize(DESCRIPTOR, value).unwrap() - n).abs() < 1e-6);
    }
    for n in [f32::NAN, f32::INFINITY, -0.01, 1.01] {
        assert!(log.denormalize(DESCRIPTOR, n).is_err());
    }
    for value in [f32::NAN, f32::INFINITY, 0.0, 101.0] {
        assert!(
            log.normalize(DESCRIPTOR, ParameterValue::Float(value))
                .is_err()
        );
    }
    let bool_descriptor = ParameterDescriptor {
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        ..DESCRIPTOR
    };
    let discrete = ParameterNormalization::Discrete;
    assert_eq!(
        discrete.denormalize(bool_descriptor, 0.499).unwrap(),
        ParameterValue::Bool(false)
    );
    assert_eq!(
        discrete.denormalize(bool_descriptor, 0.5).unwrap(),
        ParameterValue::Bool(true)
    );
    let choices = ParameterDescriptor {
        kind: ParameterKind::Choice {
            labels: &["a", "b", "c"],
        },
        default: ParameterValue::Choice(0),
        ..DESCRIPTOR
    };
    assert_eq!(
        discrete.denormalize(choices, 0.25).unwrap(),
        ParameterValue::Choice(1)
    );
    let singleton = ParameterDescriptor {
        kind: ParameterKind::Float {
            min: 2.0,
            max: 2.0,
            unit: None,
        },
        default: ParameterValue::Float(2.0),
        ..DESCRIPTOR
    };
    assert_eq!(log.normalize(singleton, singleton.default).unwrap(), 0.0);
    assert_eq!(log.denormalize(singleton, 0.7).unwrap(), singleton.default);
    let singleton = ParameterDescriptor {
        kind: ParameterKind::Choice { labels: &["a"] },
        default: ParameterValue::Choice(0),
        ..DESCRIPTOR
    };
    assert_eq!(
        discrete.normalize(singleton, singleton.default).unwrap(),
        0.0
    );
    assert_eq!(
        discrete.denormalize(singleton, 0.7).unwrap(),
        singleton.default
    );
    let mut invalid = semantics();
    invalid.stepping = ParameterStepping::Increment(f32::NAN);
    assert!(invalid.validate(DESCRIPTOR).is_err());
    let mut zero = DESCRIPTOR;
    zero.kind = ParameterKind::Float {
        min: 0.0,
        max: 100.0,
        unit: None,
    };
    assert!(semantics().validate(zero).is_err());
}

#[test]
fn linear_denormalization_preserves_asymmetric_extreme_endpoints() {
    let descriptor = ParameterDescriptor {
        kind: ParameterKind::Float {
            min: -f32::MAX,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        ..DESCRIPTOR
    };
    assert_eq!(
        ParameterNormalization::Linear
            .denormalize(descriptor, 0.0)
            .unwrap(),
        ParameterValue::Float(-f32::MAX)
    );
    assert_eq!(
        ParameterNormalization::Linear
            .denormalize(descriptor, 1.0)
            .unwrap(),
        ParameterValue::Float(1.0)
    );
}

const ATOMIC_DESCRIPTORS: &[ParameterDescriptor] = &[
    DESCRIPTOR,
    ParameterDescriptor {
        id: ParameterId::from_static(2),
        ..DESCRIPTOR
    },
];
struct AtomicDefinition {
    patches: std::sync::atomic::AtomicUsize,
    second: ParameterSemantics,
}
impl NativeDeviceMetadata for AtomicDefinition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("fixture.semantics")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        ATOMIC_DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Definition { declaration: None }.default_state()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Definition { declaration: None }.migrate_state(state)
    }
    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Ok(DESCRIPTOR.default)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        _id: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.patches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(state.clone())
    }
    fn parameter_semantics_for_state(
        &self,
        _state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        Ok(if id == ParameterId::from_static(2) {
            Some(self.second)
        } else {
            None
        })
    }
}
#[test]
fn atomic_metadata_preflights_all_access_and_semantics_before_any_private_patch() {
    let mut read_only = semantics();
    read_only.access = ParameterAccess::ReadOnly;
    let mut invalid = semantics();
    invalid.stepping = ParameterStepping::Choice;
    for second in [read_only, invalid] {
        let definition = AtomicDefinition {
            patches: std::sync::atomic::AtomicUsize::new(0),
            second,
        };
        let source = definition.default_state().unwrap();
        let snapshot = DeviceParameterSnapshot::new(&[
            (ID, DESCRIPTOR.default),
            (ParameterId::from_static(2), DESCRIPTOR.default),
        ])
        .unwrap();
        assert!(definition.patch_parameters(&source, &snapshot).is_err());
        assert_eq!(
            definition
                .patches
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }
}
