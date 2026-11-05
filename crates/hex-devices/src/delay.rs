//! Original native stereo delay. State/scalar laws and DSP history stay provider-owned.
//! Browser/UI/history use the existing registry and canonical command contracts.
//! See state and runtime modules for exact laws and bounded storage; no acoustic
//! or proprietary algorithm identity with the reference is claimed.
use hex_lattice::{
    NativeDeviceDefinition, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};
use std::{any::Any, sync::Arc};
mod state;
pub use state::*;
use state::{Settings, decode};
pub struct DelayDefinition;
pub static DEFINITION: DelayDefinition = DelayDefinition;
impl NativeDeviceDefinition for DelayDefinition {
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
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        if !(8000..=192000).contains(&context.sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(Factory {
            settings,
            rate: context.sample_rate,
        }))))
    }
}
mod filter;
mod lfo;
mod runtime;
use runtime::Factory;
#[cfg(test)]
mod tests;

/// Control-side original filter response on a logarithmic 20 Hz–Nyquist-safe
/// axis. The native panel paints these same provider coefficients, never a
/// decorative approximation or a fabricated live analyzer.
#[derive(Clone, Debug, PartialEq)]
pub struct DelayFilterPreview {
    pub points: [[f32; 2]; 129],
    pub position: [f32; 2],
    pub enabled: bool,
}
pub fn filter_preview(
    state: &NativeDeviceState,
    rate: u32,
) -> Result<DelayFilterPreview, DeviceContractError> {
    if !(8000..=192000).contains(&rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let s = decode(state)?;
    let maximum = 20000.0_f64.min(f64::from(rate) * 0.45);
    let log = (maximum / 20.0).ln();
    let mut filter = filter::BandPass::default();
    filter.tune(
        f64::from(s.value(FREQUENCY)),
        f64::from(s.value(WIDTH)),
        rate,
    );
    Ok(DelayFilterPreview {
        points: std::array::from_fn(|i| {
            let x = i as f64 / 128.0;
            let frequency = 20.0 * (log * x).exp();
            let db = 20.0 * filter.magnitude(frequency, rate).max(1e-8).log10();
            [x as f32, ((db + 48.0) / 60.0).clamp(0.0, 1.0) as f32]
        }),
        position: [
            ((f64::from(s.value(FREQUENCY)) / 20.0).ln() / log).clamp(0.0, 1.0) as f32,
            (s.value(WIDTH) - 0.5) / 8.5,
        ],
        enabled: s.flag(FILTER),
    })
}
/// Inverse of the panel's exact logarithmic frequency/linear octave-width axes.
pub fn filter_values(position: [f32; 2], rate: u32) -> Result<[f32; 2], DeviceContractError> {
    if !(8000..=192000).contains(&rate)
        || position
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let maximum = 20000.0_f64.min(f64::from(rate) * 0.45);
    Ok([
        (20.0 * ((maximum / 20.0).ln() * f64::from(position[0])).exp()) as f32,
        0.5 + position[1] * 8.5,
    ])
}
