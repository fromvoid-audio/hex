//! Stable neutral automation targets. Host trims are separate from provider
//! ParameterId space: their identity survives a provider change on one device.
use crate::{DeviceContractError, ParameterId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceAutomationTarget {
    Parameter(ParameterId),
    HostInputGain,
    HostOutputGain,
}
impl DeviceAutomationTarget {
    pub fn validate(self) -> Result<(), DeviceContractError> {
        match self {
            Self::Parameter(parameter) => parameter.validate(),
            Self::HostInputGain | Self::HostOutputGain => Ok(()),
        }
    }
    pub const fn provider_parameter(self) -> Option<ParameterId> {
        match self {
            Self::Parameter(parameter) => Some(parameter),
            Self::HostInputGain | Self::HostOutputGain => None,
        }
    }
    pub const fn is_host(self) -> bool {
        matches!(self, Self::HostInputGain | Self::HostOutputGain)
    }
}
