//! Control-side semantic declarations. None means undeclared, never guessed.
use crate::{
    DeviceContractError, DeviceParameterSnapshot, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterValue, validated_state_descriptors,
};

macro_rules! semantic_enum {
    ($name:ident {$($variant:ident => $label:literal),+ $(,)?}) => {
        #[derive(Clone,Copy,Debug,PartialEq,Eq)]
        pub enum $name {$($variant),+}
        impl $name {pub const fn label(self)->&'static str {match self {$(Self::$variant=>$label),+}}}
    };
}
semantic_enum!(ParameterAccess {ReadWrite=>"read/write",ReadOnly=>"read-only"});
semantic_enum!(ParameterNormalization {Unspecified=>"undeclared",Linear=>"linear",PositiveLogarithmic=>"positive logarithmic",Discrete=>"discrete"});
semantic_enum!(ParameterAutomation {Undeclared=>"undeclared",Unsupported=>"unsupported",DeviceArrangementEnvelope=>"device arrangement envelope"});
semantic_enum!(ParameterModulation {Undeclared=>"undeclared",Unsupported=>"unsupported",SampleRate=>"sample rate"});
semantic_enum!(ParameterSmoothing {Unspecified=>"undeclared",None=>"none",DeviceManaged=>"device managed"});
semantic_enum!(ParameterChangeImpact {Unspecified=>"undeclared",ValueOnly=>"value only",Structural=>"structural"});
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterStepping {
    Unspecified,
    Continuous,
    Boolean,
    Choice,
    Increment(f32),
}
impl ParameterStepping {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "undeclared",
            Self::Continuous => "continuous",
            Self::Boolean => "boolean",
            Self::Choice => "choice",
            Self::Increment(_) => "canonical increment",
        }
    }
}
/// Canonical units/defaults remain in the descriptor. This declaration grants
/// no generic runtime delivery, direct callback mutation or UI authoring path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterSemantics {
    pub access: ParameterAccess,
    pub normalization: ParameterNormalization,
    pub stepping: ParameterStepping,
    pub automation: ParameterAutomation,
    pub modulation: ParameterModulation,
    pub smoothing: ParameterSmoothing,
    pub change_impact: ParameterChangeImpact,
}
impl ParameterSemantics {
    pub fn validate(self, descriptor: ParameterDescriptor) -> Result<(), DeviceContractError> {
        descriptor.validate_descriptor()?;
        let normalized = match (self.normalization, descriptor.kind) {
            (ParameterNormalization::Unspecified, _) => true,
            (ParameterNormalization::Linear, ParameterKind::Float { .. }) => true,
            (ParameterNormalization::PositiveLogarithmic, ParameterKind::Float { min, .. }) => {
                min > 0.0
            }
            (
                ParameterNormalization::Discrete,
                ParameterKind::Bool | ParameterKind::Choice { .. },
            ) => true,
            _ => false,
        };
        let stepped = match (self.stepping, descriptor.kind) {
            (ParameterStepping::Unspecified, _) => true,
            (ParameterStepping::Continuous, ParameterKind::Float { .. }) => true,
            (ParameterStepping::Boolean, ParameterKind::Bool) => true,
            (ParameterStepping::Choice, ParameterKind::Choice { .. }) => true,
            (ParameterStepping::Increment(step), ParameterKind::Float { min, max, .. }) => {
                step.is_finite() && step > 0.0 && f64::from(step) <= f64::from(max) - f64::from(min)
            }
            _ => false,
        };
        if !normalized
            || !stepped
            || (self.access == ParameterAccess::ReadOnly
                && self.automation == ParameterAutomation::DeviceArrangementEnvelope)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        Ok(())
    }
}

/// Current codec-validated state is required, exactly as for state descriptors.
/// Unknown/deleted parameters refuse before calling an override. No migration,
/// defaults, registry recursion or callback work occurs here.
pub fn validated_parameter_semantics<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    id: ParameterId,
) -> Result<Option<ParameterSemantics>, DeviceContractError> {
    let descriptors = validated_state_descriptors(definition, state)?;
    let descriptor = *descriptors
        .iter()
        .find(|d| d.id == id)
        .ok_or(DeviceContractError::UnknownParameter)?;
    semantics_for_descriptor(definition, state, descriptor)
}

fn semantics_for_descriptor<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    descriptor: ParameterDescriptor,
) -> Result<Option<ParameterSemantics>, DeviceContractError> {
    let semantics = definition.parameter_semantics_for_state(state, descriptor.id)?;
    if let Some(value) = semantics {
        value.validate(descriptor)?;
    }
    Ok(semantics)
}

fn require_descriptor_writable<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    descriptor: ParameterDescriptor,
) -> Result<(), DeviceContractError> {
    if semantics_for_descriptor(definition, state, descriptor)?
        .is_some_and(|s| s.access == ParameterAccess::ReadOnly)
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}

/// Validate every value and access declaration before any private batch patch.
/// The complete current descriptor set is enumerated once for this preflight;
/// private sequential candidates still require fresh validation before mutation.
pub fn validate_parameter_snapshot_for_write<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    snapshot: &DeviceParameterSnapshot,
) -> Result<(), DeviceContractError> {
    let descriptors = validated_state_descriptors(definition, state)?;
    snapshot.validate(&descriptors)?;
    for &(id, _) in snapshot.entries() {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        require_descriptor_writable(definition, state, *descriptor)?;
    }
    Ok(())
}

/// Validate one write against its current private candidate, including dynamic
/// membership, value domain and access. No earlier state's metadata is reused.
pub fn validate_parameter_write<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    id: ParameterId,
    value: ParameterValue,
) -> Result<(), DeviceContractError> {
    let descriptors = validated_state_descriptors(definition, state)?;
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.id == id)
        .ok_or(DeviceContractError::UnknownParameter)?;
    descriptor.validate(value)?;
    require_descriptor_writable(definition, state, *descriptor)
}

/// Enumerate once on control, retaining real stable parameter identities.
pub fn validated_state_parameters<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
) -> Result<Vec<(ParameterDescriptor, Option<ParameterSemantics>)>, DeviceContractError> {
    validated_state_descriptors(definition, state)?
        .into_iter()
        .map(|descriptor| {
            Ok((
                descriptor,
                semantics_for_descriptor(definition, state, descriptor)?,
            ))
        })
        .collect()
}
impl ParameterNormalization {
    /// Refuse out-of-domain/nonfinite input. Singleton domains normalize to zero.
    pub fn normalize(
        self,
        descriptor: ParameterDescriptor,
        value: ParameterValue,
    ) -> Result<f32, DeviceContractError> {
        descriptor.validate_descriptor()?;
        descriptor.validate(value)?;
        match (self, descriptor.kind, value) {
            (Self::Linear, ParameterKind::Float { min, max, .. }, ParameterValue::Float(v)) => {
                Ok(if max == min {
                    0.0
                } else {
                    ((f64::from(v) - f64::from(min)) / (f64::from(max) - f64::from(min))) as f32
                })
            }
            (
                Self::PositiveLogarithmic,
                ParameterKind::Float { min, max, .. },
                ParameterValue::Float(v),
            ) if min > 0.0 => Ok(if max == min {
                0.0
            } else {
                ((f64::from(v) / f64::from(min)).ln() / (f64::from(max) / f64::from(min)).ln())
                    as f32
            }),
            (Self::Discrete, ParameterKind::Bool, ParameterValue::Bool(v)) => {
                Ok(f32::from(u8::from(v)))
            }
            (Self::Discrete, ParameterKind::Choice { labels }, ParameterValue::Choice(v)) => {
                Ok(if labels.len() == 1 {
                    0.0
                } else {
                    v as f32 / (labels.len() - 1) as f32
                })
            }
            _ => Err(DeviceContractError::InvalidDescriptor),
        }
    }
    /// Boolean midpoint is true; choices use nearest index with midpoint upward.
    /// All normalized values in a singleton domain decode to its sole value.
    pub fn denormalize(
        self,
        descriptor: ParameterDescriptor,
        n: f32,
    ) -> Result<ParameterValue, DeviceContractError> {
        descriptor.validate_descriptor()?;
        if !n.is_finite() || !(0.0..=1.0).contains(&n) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let value = match (self, descriptor.kind) {
            (Self::Linear, ParameterKind::Float { min, max, .. }) => {
                ParameterValue::Float(if n == 0.0 {
                    min
                } else if n == 1.0 {
                    max
                } else {
                    (f64::from(min) * (1.0 - f64::from(n)) + f64::from(max) * f64::from(n)) as f32
                })
            }
            (Self::PositiveLogarithmic, ParameterKind::Float { min, max, .. }) if min > 0.0 => {
                ParameterValue::Float(if n == 0.0 {
                    min
                } else if n == 1.0 {
                    max
                } else {
                    (f64::from(min) * (f64::from(max) / f64::from(min)).powf(f64::from(n))) as f32
                })
            }
            (Self::Discrete, ParameterKind::Bool) => ParameterValue::Bool(n >= 0.5),
            (Self::Discrete, ParameterKind::Choice { labels }) => {
                ParameterValue::Choice((n * (labels.len() - 1) as f32).round() as u32)
            }
            _ => return Err(DeviceContractError::InvalidDescriptor),
        };
        descriptor.validate(value)?;
        Ok(value)
    }
}

/// Guard a validated edit boundary before calling a device override. Inactive
/// authored controls remain writable; read-only access is a separate contract.
pub fn require_parameter_writable<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    id: ParameterId,
) -> Result<(), DeviceContractError> {
    if validated_parameter_semantics(definition, state, id)?
        .is_some_and(|s| s.access == ParameterAccess::ReadOnly)
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}
