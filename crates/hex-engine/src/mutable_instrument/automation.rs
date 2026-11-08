//! Borrowed parameter readings inside the existing source transaction.
//!
//! The caller owns the program and its Arrangement-content clock. Readings are
//! independent of the musical clock sent to DSP and are never retained here.
//! Volume subdivision shifts scalar and bulk readings together. The same twin
//! processes parameters, exact-ID gains and MIDI; there is no second voice owner.
use super::*;
use hex_lattice::NativeParameterAutomation;
use hex_lattice_contract::{DeviceContractError, ParameterId, ParameterValue};

pub(super) fn validate_width(
    values: Option<&dyn NativeParameterAutomation>,
    frames: usize,
) -> Result<(), MutableInstrumentRuntimeError> {
    if values.is_some() && frames > hex_lattice::MAX_INSTRUMENT_AUTOMATION_FRAMES {
        return Err(map_runtime_error(
            NativeInstrumentRuntimeError::InvalidParameterAutomation,
        ));
    }
    Ok(())
}

pub(super) struct OffsetValues<'a> {
    pub(super) values: &'a dyn NativeParameterAutomation,
    pub(super) offset: usize,
}
impl NativeParameterAutomation for OffsetValues<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.values.parameter_ids()
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        self.values.value_at(id, self.offset.checked_add(offset)?)
    }
    fn values_at(
        &self,
        offset: usize,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        self.values.values_at(
            self.offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
            output,
        )
    }
}

#[cfg(test)]
mod tests;
