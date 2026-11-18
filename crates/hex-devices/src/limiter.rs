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
pub use runtime::structural::{PreparedStructuralLimiter, Runtime as StructuralLimiterRuntime};
pub use runtime::{Factory as PreparedLimiter, Runtime as LimiterRuntime};
#[cfg(test)]
mod tests;

pub struct LimiterDefinition;
pub static DEFINITION: LimiterDefinition = LimiterDefinition;
impl LimiterDefinition {
    /// Prepare the original fixed-layout owned transaction without creating a runtime.
    /// Empty parameters select the default factory; otherwise IDs must be sorted,
    /// unique value controls. Structural controls require
    /// `prepare_owned_structural_for_parameters`. The host remains responsible
    /// for source admission and retains the returned factory and runtime.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[hex_lattice_contract::ParameterId],
    ) -> Result<PreparedLimiter, DeviceContractError> {
        automation::prepare_fixed_factory(state, rate, parameters)
    }
    /// Prepare an exact owned lookahead/routing/mode union at fixed maximum latency.
    /// Requires at least one structural target plus optional value controls.
    /// Native metadata grants all three selectors; preparation transfers no
    /// source lease or resource custody.
    pub fn prepare_owned_structural_for_parameters(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[hex_lattice_contract::ParameterId],
    ) -> Result<PreparedStructuralLimiter, DeviceContractError> {
        PreparedStructuralLimiter::new(state, rate, parameters)
    }
}
impl NativeDeviceDefinition for LimiterDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(matches!(
            id,
            parameter::LOOKAHEAD | parameter::ROUTING | parameter::MODE
        ) || automation::slot(id).is_some())
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
