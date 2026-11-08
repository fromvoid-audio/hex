//! Neutral source identity; owning device context remains a separate identity.
use crate::DeviceContractError;

/// A stable nonzero modulator identity scoped by its owning device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulationSourceId(u64);

impl ModulationSourceId {
    pub const fn new(value: u64) -> Result<Self, DeviceContractError> {
        if value == 0 {
            return Err(DeviceContractError::InvalidId);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_nonzero_and_round_trips() {
        assert_eq!(
            ModulationSourceId::new(0),
            Err(DeviceContractError::InvalidId)
        );
        assert_eq!(ModulationSourceId::new(u64::MAX).unwrap().value(), u64::MAX);
    }
}
