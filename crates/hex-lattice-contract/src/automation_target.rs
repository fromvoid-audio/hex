//! Stable neutral automation targets. Host trims are separate from provider
//! ParameterId space: their identity survives a provider change on one device.
use crate::{DeviceContractError, ParameterId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceAutomationTarget {
    Parameter(ParameterId),
    HostInputGain,
    HostOutputGain,
    /// Raw SDK ID, including zero, scoped to the exact VST3 class.
    Vst3 {
        class: [u8; 16],
        parameter: u32,
    },
}
impl DeviceAutomationTarget {
    pub fn validate(self) -> Result<(), DeviceContractError> {
        match self {
            Self::Parameter(parameter) => parameter.validate(),
            Self::HostInputGain | Self::HostOutputGain => Ok(()),
            Self::Vst3 { class, parameter } if class != [0; 16] && parameter <= i32::MAX as u32 => {
                Ok(())
            }
            Self::Vst3 { .. } => Err(DeviceContractError::InvalidId),
        }
    }
    pub const fn provider_parameter(self) -> Option<ParameterId> {
        match self {
            Self::Parameter(parameter) => Some(parameter),
            Self::HostInputGain | Self::HostOutputGain | Self::Vst3 { .. } => None,
        }
    }
    pub const fn is_host(self) -> bool {
        matches!(self, Self::HostInputGain | Self::HostOutputGain)
    }
}
