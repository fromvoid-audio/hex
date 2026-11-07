//! Native lookahead limiter with independently authored dynamics laws.
//!
//! The provider owns its versioned state, bounded owned transaction histories and
//! committed gain-reduction readings. The host owns graph admission, latency
//! compensation, metering endpoints and retirement. Control inventory follows
//! Live 12; proprietary transfer/release laws and acoustic identity are unverified.
use hex_lattice::{NativeDeviceDefinition, PreparedProcessorFactory, StereoMatrix};
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};
use std::sync::Arc;
mod state;
use state::decode;
pub use state::{STATE_VERSION, TYPE_ID, parameter};
mod runtime;
#[cfg(test)]
mod tests;

pub struct LimiterDefinition;
pub static DEFINITION: LimiterDefinition = LimiterDefinition;
impl NativeDeviceDefinition for LimiterDefinition {
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
