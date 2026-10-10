//! Bounded metadata and authored state shared across native device boundaries.
use std::{fmt, sync::Arc};

pub const MAX_NATIVE_DEVICE_STATE_BYTES: usize = 4_096;
pub const MAX_DEVICE_TYPE_ID_BYTES: usize = 128;
pub const MAX_PARAMETER_ID: u32 = 65_535;
pub const MAX_NATIVE_DEVICE_TYPES: usize = 256;
pub const MAX_PARAMETERS_PER_DEVICE: usize = 256;
pub const MAX_PARAMETER_LABEL_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceTypeId {
    bytes: [u8; MAX_DEVICE_TYPE_ID_BYTES],
    length: u8,
    valid: bool,
}
impl DeviceTypeId {
    pub fn new(value: &str) -> Result<Self, DeviceContractError> {
        if !Self::is_valid(value) {
            return Err(DeviceContractError::InvalidId);
        }
        let mut bytes = [0; MAX_DEVICE_TYPE_ID_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            length: value.len() as u8,
            valid: true,
        })
    }
    /// Static built-in identifier; state and registry boundaries still validate it.
    pub const fn from_static(value: &str) -> Self {
        let source = value.as_bytes();
        let mut bytes = [0; MAX_DEVICE_TYPE_ID_BYTES];
        let mut index = 0;
        while index < source.len() && index < MAX_DEVICE_TYPE_ID_BYTES {
            bytes[index] = source[index];
            index += 1;
        }
        Self {
            bytes,
            length: if source.len() <= MAX_DEVICE_TYPE_ID_BYTES {
                source.len() as u8
            } else {
                0
            },
            valid: source.len() <= MAX_DEVICE_TYPE_ID_BYTES,
        }
    }
    pub fn as_str(&self) -> Result<&str, DeviceContractError> {
        if !self.valid {
            return Err(DeviceContractError::InvalidId);
        }
        std::str::from_utf8(&self.bytes[..usize::from(self.length)])
            .map_err(|_| DeviceContractError::InvalidId)
    }
    pub fn validate(self) -> Result<(), DeviceContractError> {
        Self::is_valid(self.as_str()?)
            .then_some(())
            .ok_or(DeviceContractError::InvalidId)
    }
    fn is_valid(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= MAX_DEVICE_TYPE_ID_BYTES
            && value.contains('.')
            && !value.starts_with('.')
            && !value.ends_with('.')
            && !value.contains("..")
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterId(u32);
impl ParameterId {
    pub const fn new(value: u32) -> Result<Self, DeviceContractError> {
        if value == 0 || value > MAX_PARAMETER_ID {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }
    /// Const constructor for statically declared built-in parameter identifiers.
    pub const fn from_static(value: u32) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u32 {
        self.0
    }
    pub const fn validate(self) -> Result<(), DeviceContractError> {
        if self.0 == 0 || self.0 > MAX_PARAMETER_ID {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterValue {
    Float(f32),
    Bool(bool),
    Choice(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterKind {
    Float {
        min: f32,
        max: f32,
        /// Unit of the canonical value and range, before any presentation mapping.
        unit: Option<&'static str>,
    },
    Bool,
    Choice {
        labels: &'static [&'static str],
    },
}

/// How a parameter's canonical value is presented to generic controls.
/// This changes UI input/display only; descriptors, edits and persisted values
/// remain in the domain and unit declared by [`ParameterKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterPresentation {
    /// Display and edit the canonical value directly.
    #[default]
    Linear,
    /// Canonical values are linear amplitudes presented and entered in dB.
    LinearAmplitudeDecibels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterDescriptor {
    pub id: ParameterId,
    pub label: &'static str,
    pub kind: ParameterKind,
    pub default: ParameterValue,
    pub presentation: ParameterPresentation,
}
impl ParameterDescriptor {
    pub fn validate_descriptor(self) -> Result<(), DeviceContractError> {
        self.id.validate()?;
        if self.label.is_empty()
            || self.label.len() > MAX_PARAMETER_LABEL_BYTES
            || !self.label.bytes().all(|byte| !byte.is_ascii_uppercase())
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if self.presentation == ParameterPresentation::LinearAmplitudeDecibels
            && !matches!(self.kind, ParameterKind::Float { min, max, .. } if min >= 0.0 && max > min)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        match self.kind {
            ParameterKind::Float { min, max, unit }
                if !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || unit.is_some_and(|value| {
                        value.bytes().any(|byte| byte.is_ascii_uppercase())
                    }) =>
            {
                Err(DeviceContractError::InvalidDescriptor)
            }
            ParameterKind::Choice { labels }
                if labels.is_empty()
                    || labels.iter().any(|label| {
                        label.is_empty() || label.bytes().any(|byte| byte.is_ascii_uppercase())
                    }) =>
            {
                Err(DeviceContractError::InvalidDescriptor)
            }
            _ => self.validate(self.default),
        }
    }
    pub fn validate(self, value: ParameterValue) -> Result<(), DeviceContractError> {
        let valid = match (self.kind, value, self.default) {
            (ParameterKind::Float { min, max, .. }, ParameterValue::Float(v), _) => {
                v.is_finite() && v >= min && v <= max
            }
            (ParameterKind::Bool, ParameterValue::Bool(_), _) => true,
            (ParameterKind::Choice { labels }, ParameterValue::Choice(v), _) => {
                !labels.is_empty() && (v as usize) < labels.len()
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(DeviceContractError::InvalidParameterValue)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDeviceState {
    type_id: DeviceTypeId,
    state_version: u32,
    payload: Arc<[u8]>,
}
impl NativeDeviceState {
    pub fn new(
        type_id: DeviceTypeId,
        state_version: u32,
        payload: Vec<u8>,
    ) -> Result<Self, DeviceContractError> {
        type_id.validate()?;
        if state_version == 0 {
            return Err(DeviceContractError::InvalidStateVersion);
        }
        if payload.len() > MAX_NATIVE_DEVICE_STATE_BYTES {
            return Err(DeviceContractError::StateTooLarge);
        }
        Ok(Self {
            type_id,
            state_version,
            payload: Arc::from(payload),
        })
    }
    pub const fn type_id(&self) -> DeviceTypeId {
        self.type_id
    }
    pub const fn state_version(&self) -> u32 {
        self.state_version
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceContractError {
    InvalidId,
    InvalidStateVersion,
    StateTooLarge,
    WrongDeviceType,
    UnsupportedStateVersion,
    MalformedState,
    UnknownParameter,
    InvalidParameterValue,
    InvalidDescriptor,
    DuplicateDeviceType,
    RuntimeBudgetExceeded,
    RuntimeAllocation,
    UnsupportedProcessingClass,
}
impl fmt::Display for DeviceContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::InvalidId => "device contract id is outside its valid range",
                Self::InvalidStateVersion => "device state version must be non-zero",
                Self::StateTooLarge => "native device state exceeds its size limit",
                Self::WrongDeviceType => "device state has the wrong type",
                Self::UnsupportedStateVersion => "device state version is unsupported",
                Self::MalformedState => "native device state is malformed",
                Self::UnknownParameter => "device parameter is unknown",
                Self::InvalidParameterValue => "device parameter value is invalid",
                Self::InvalidDescriptor => "native device parameter descriptor is invalid",
                Self::DuplicateDeviceType => "native device registry contains a duplicate type",
                Self::RuntimeBudgetExceeded =>
                    "native processor runtime exceeds its declared budget",
                Self::RuntimeAllocation => "native processor runtime allocation failed",
                Self::UnsupportedProcessingClass =>
                    "native device processing class is not admitted",
            }
        )
    }
}
impl std::error::Error for DeviceContractError {}

/// Device metadata/state operations; implementations own their codec.
pub trait NativeDeviceMetadata: Sync {
    fn type_id(&self) -> DeviceTypeId;
    /// Optional lowercase presentation name. The type identifier remains the
    /// stable storage/registry identity; unnamed definitions display that ID.
    fn display_name(&self) -> Option<&'static str> {
        None
    }
    fn state_version(&self) -> u32;
    fn descriptors(&self) -> &'static [ParameterDescriptor];
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError>;
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError>;
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError>;
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaced_ids_state_versions_and_payloads_are_bounded() {
        assert!(DeviceTypeId::new("hex.utility").is_ok());
        for bad in [
            "",
            "utility",
            ".vendor.device",
            "vendor.device.",
            "vendor..device",
            "Vendor.device",
            "vendor.device/other",
        ] {
            assert!(
                DeviceTypeId::new(bad).is_err(),
                "accepted invalid id {bad:?}"
            );
        }
        assert!(DeviceTypeId::from_static("Bad.ID").validate().is_err());
        let id = DeviceTypeId::new("hex.utility").unwrap();
        assert!(NativeDeviceState::new(id, 0, vec![]).is_err());
        assert!(NativeDeviceState::new(id, 1, vec![0; MAX_NATIVE_DEVICE_STATE_BYTES + 1]).is_err());
        let state = NativeDeviceState::new(id, 1, vec![1, 2]).unwrap();
        let clone = state.clone();
        assert!(Arc::ptr_eq(&state.payload, &clone.payload));
    }

    #[test]
    fn descriptors_reject_bad_ranges_defaults_choices_and_case() {
        let id = ParameterId::new(1).unwrap();
        let good = ParameterDescriptor {
            id,
            label: "gain",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 2.0,
                unit: Some("linear"),
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Linear,
        };
        assert!(good.validate_descriptor().is_ok());
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Float {
                    min: f32::NAN,
                    max: 1.0,
                    unit: None
                },
                ..good
            }
            .validate_descriptor()
            .is_err()
        );

        assert!(
            ParameterDescriptor {
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_ok()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Float {
                    min: -0.1,
                    max: 2.0,
                    unit: Some("db"),
                },
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Bool,
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                default: ParameterValue::Float(3.0),
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                label: "Gain",
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Choice {
                    labels: &["a", "b"]
                },
                default: ParameterValue::Choice(2),
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
    }
}
