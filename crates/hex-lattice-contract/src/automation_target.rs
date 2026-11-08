//! Stable neutral automation targets. Host trims are separate from provider
//! ParameterId space: their identity survives a provider change on one device.
use crate::{DeviceContractError, ParameterId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceAutomationTarget {
    Parameter(ParameterId),
    HostInputGain,
    HostOutputGain,
    /// Raw SDK ID, including zero. The authored lane owns the class stamp;
    /// this key stays small for native callback commands and receipts.
    Vst3 {
        parameter: u32,
    },
}
impl DeviceAutomationTarget {
    pub fn validate(self) -> Result<(), DeviceContractError> {
        match self {
            Self::Parameter(parameter) => parameter.validate(),
            Self::HostInputGain | Self::HostOutputGain => Ok(()),
            Self::Vst3 { parameter } if parameter <= i32::MAX as u32 => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_vst3_parameter_keys_fit_the_callback_target_budget() {
        assert!(std::mem::size_of::<DeviceAutomationTarget>() <= 8);
        let zero = DeviceAutomationTarget::Vst3 { parameter: 0 };
        assert!(zero.validate().is_ok());
        assert!(!zero.is_host());
        assert!(zero.provider_parameter().is_none());
        assert!(
            DeviceAutomationTarget::Vst3 {
                parameter: u32::MAX
            }
            .validate()
            .is_err()
        );
    }
}
