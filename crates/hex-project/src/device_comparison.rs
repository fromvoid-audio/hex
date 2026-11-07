//! Shared authored comparison state; no DSP, PCM or runtime ownership.
use crate::{Device, Invalid};
use hex_lattice_contract::NativeDeviceState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceVariant {
    A,
    B,
}
impl DeviceVariant {
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceComparisonAction {
    Switch,
    CopyToInactive,
}

/// The other authored variant. `Device::state()` remains the active provider input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceComparison {
    active: DeviceVariant,
    inactive: NativeDeviceState,
}
impl DeviceComparison {
    pub fn new(active: DeviceVariant, inactive: NativeDeviceState) -> Self {
        Self { active, inactive }
    }
    pub fn active(&self) -> DeviceVariant {
        self.active
    }
    pub fn inactive(&self) -> &NativeDeviceState {
        &self.inactive
    }
    pub fn with_global_trims_from(mut self, state: &NativeDeviceState) -> Result<Self, Invalid> {
        self.inactive = self
            .inactive
            .with_input_gain_millidb(state.input_gain_millidb())
            .and_then(|inactive| inactive.with_output_gain_millidb(state.output_gain_millidb()))
            .map_err(|_| Invalid("invalid device comparison host gains"))?;
        Ok(self)
    }
    pub(crate) fn validate(&self, state: &NativeDeviceState) -> Result<(), Invalid> {
        if self.inactive.type_id() != state.type_id() {
            return Err(Invalid(
                "device comparison variants must use the same provider",
            ));
        }
        if self.inactive.input_gain_millidb() != state.input_gain_millidb()
            || self.inactive.output_gain_millidb() != state.output_gain_millidb()
        {
            return Err(Invalid(
                "device comparison host gains must be device-global",
            ));
        }
        Ok(())
    }
}
impl Device {
    pub fn comparison(&self) -> Option<&DeviceComparison> {
        self.comparison.as_ref()
    }
    pub fn active_variant(&self) -> DeviceVariant {
        self.comparison
            .as_ref()
            .map_or(DeviceVariant::A, DeviceComparison::active)
    }
    /// Actual borrowed authored fields, including inactive asset bindings. No allocation.
    pub fn authored_states(&self) -> impl Iterator<Item = &NativeDeviceState> {
        std::iter::once(&self.state).chain(self.comparison.iter().map(DeviceComparison::inactive))
    }
    pub(crate) fn replace_state(&mut self, state: NativeDeviceState) -> Result<(), Invalid> {
        self.validate_automation_provider(&state)?;
        self.comparison = if self.state.type_id() == state.type_id() {
            self.comparison
                .clone()
                .map(|comparison| comparison.with_global_trims_from(&state))
                .transpose()?
        } else {
            None
        };
        self.state = state;
        Ok(())
    }
    pub(crate) fn compare(
        &mut self,
        expected: DeviceVariant,
        action: DeviceComparisonAction,
    ) -> Result<(), Invalid> {
        if self.active_variant() != expected {
            return Err(Invalid(
                "device comparison changed; retry from its current slot",
            ));
        }
        match action {
            DeviceComparisonAction::CopyToInactive => {
                // Absent B already means an identical copy; keep this a true no-op.
                if let Some(comparison) = self.comparison.as_mut() {
                    comparison.inactive.clone_from(&self.state);
                }
            }
            DeviceComparisonAction::Switch => {
                if let Some(comparison) = self.comparison.as_mut() {
                    std::mem::swap(&mut self.state, &mut comparison.inactive);
                    comparison.active = comparison.active.other();
                } else {
                    self.comparison =
                        Some(DeviceComparison::new(DeviceVariant::B, self.state.clone()));
                }
            }
        }
        Ok(())
    }
}
