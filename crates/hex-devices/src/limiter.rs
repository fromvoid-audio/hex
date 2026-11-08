//! Native lookahead limiter with independently authored dynamics laws.
//!
//! The provider owns its versioned state, bounded owned transaction histories and
//! committed gain-reduction readings. The host owns graph admission, latency
//! compensation, metering endpoints and retirement. Control inventory follows
//! Live 12; proprietary transfer/release laws and acoustic identity are unverified.
use hex_lattice::{NativeDeviceDefinition, PreparedProcessorFactory, StereoMatrix};
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};
use std::sync::Arc;
mod automation;
mod state;
use state::decode;
pub use state::{STATE_VERSION, TYPE_ID, parameter};
mod runtime;
pub use runtime::lookahead::PreparedLookahead;
#[cfg(test)]
mod tests;

pub struct LimiterDefinition;
pub static DEFINITION: LimiterDefinition = LimiterDefinition;
impl LimiterDefinition {
    /// Prepare exact owned lookahead eligibility with fixed maximum latency.
    /// Requires lookahead plus an optional subset of the nine value controls.
    /// Native lookahead automation selects this factory for the exact retained
    /// target union; host admission and resource custody remain separate.
    pub fn prepare_owned_lookahead_for_parameters(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[hex_lattice_contract::ParameterId],
    ) -> Result<PreparedLookahead, DeviceContractError> {
        PreparedLookahead::new(state, rate, parameters)
    }
}
impl NativeDeviceDefinition for LimiterDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(id == parameter::LOOKAHEAD || automation::slot(id).is_some())
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        ids: &[hex_lattice_contract::ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        automation::prepare(state, context, ids)
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        if !(8_000..=192_000).contains(&context.sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(
            runtime::Factory::new(decode(state)?, context.sample_rate),
        ))))
    }
}
