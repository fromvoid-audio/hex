//! Original functional room reverb: filtered early reflections and an eight-line
//! feedback diffusion network. Native controls follow the reference workflow;
//! this is not an assertion of proprietary DSP or sound parity.
//!
//! The wet network runs at sample_rate / ceil(sample_rate / 48000), with input
//! averaging and output interpolation; dry stays at host rate. Preparation owns
//! every allocation. Fixed maximum ring capacities allow all authored parameter
//! edits to retain history and ramp without callback allocation. Signal values
//! are bounded to +/-16, recursive storage to +/-32, with a 1e-20 numerical floor.
//!
//! Fast/slow/none smoothing means 20 ms/100 ms/one sample. Stop, seek and rate
//! changes reset; loop wraps continue. Bypass advances private history. Compatible
//! live bypass edits use the host's reversible 5 ms wet/dry blend and keep one
//! room owner; removal retains the separate tail policy below. Freeze
//! ramps feedback to unity; flat neutralizes decay shaping, cut suppresses new
//! excitation. Frozen rooms declare Unbounded tails under the existing host
//! policy: removed frozen tails persist until Stop, using bounded tail banks
//! and explicit admission backpressure. Unfrozen rooms hard-clear at their
//! declared zero-input deadline (including up to 100 ms of copied smoothing).
//! The shared Arrangement adapter supplies authored dry/wet at host rate.
//! Other admitted controls use tank evaluations, including decay, size and
//! decay ratios. Density and six filter/modulation/shaping switches are stepped;
//! canonical edits retain their smoothing. Freeze/flat/cut use the same typed
//! delivery. Smoothing-mode automation retimes active canonical ramps to none
//! (one sample), fast (20ms) or slow (100ms). Authored curves stay direct. Live
//! handoff inherits the effective mode only when the incoming factory admits
//! that program; lane removal restores canonical smoothing without DSP reset.
//! Fixed delay rings preserve history. With authored feedback controls, gains
//! derive from effective parameter ramps; canonical-only rendering retains
//! prepared coefficient ramps. Decay-admitted unfrozen factories reserve the
//! maximum finite tail. Ordinary factories keep their previous tail declaration
//! and refuse unprepared decay input. Factories admitting freeze input declare
//! unbounded tails before publication and otherwise refuse freeze delivery.
//! Detached tails retain their last validated values after the host freezes the
//! automation clock.

mod runtime;
mod state;
use hex_lattice::{NativeDeviceDefinition, StereoMatrix};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterValue,
};
pub use runtime::{PreparedReverb, RUNTIME_HEAP_ALLOCATIONS, ReverbRuntime, ReverbRuntimeStorage};
use state::{DESCRIPTORS, Settings, decode, encode, index, value};
pub use state::{PARAMETER_COUNT, parameter};

pub const REVERB_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.reverb");
pub const REVERB_STATE_VERSION: u32 = 3;
pub struct ReverbDefinition;
pub static REVERB_DEFINITION: ReverbDefinition = ReverbDefinition;
impl ReverbDefinition {
    /// Concrete original control constructor for native custody. Private fixed
    /// preparation fields contain no retained asset/provider/host owners.
    pub fn prepare_owned_room(
        state: &NativeDeviceState,
        rate: u32,
    ) -> Result<PreparedReverb, DeviceContractError> {
        Self::prepare_owned_room_for_parameters(state, rate, &[])
    }
    /// Exact admitted provider targets, including live base-only input slots.
    /// Decay admission expands the finite tail promise; freeze admission makes
    /// it unbounded. Both promises are established before publication. Smoothing
    /// admission permits effective-mode inheritance across live canonical edits.
    pub fn prepare_owned_room_for_parameters(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedReverb, DeviceContractError> {
        if !parameters.is_empty() {
            runtime::automation::validate_ids(parameters)?;
        }
        state.require_asset_free_processing()?;
        let state = state::migrate(state)?;
        runtime::PreparedReverb::new(rate, decode(&state)?)
            .map(|prepared| prepared.with_automation(parameters))
    }
}
impl NativeDeviceMetadata for ReverbDefinition {
    fn type_id(&self) -> DeviceTypeId {
        REVERB_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("reverb")
    }
    fn state_version(&self) -> u32 {
        REVERB_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings::default_values())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        state::migrate(state)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Ok(value(decode(state)?, index(id)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        v: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        let i = index(id)?;
        DESCRIPTORS[i].validate(v)?;
        settings.0[i] = state::scalar(v);
        encode(settings)
    }
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = decode(state)?;
        index(id)?;
        let gate = match id {
            parameter::LO_FREQ | parameter::LO_Q => Some(parameter::LO_CUT),
            parameter::HI_FREQ | parameter::HI_Q => Some(parameter::HI_CUT),
            parameter::SPIN_AMOUNT | parameter::SPIN_RATE => Some(parameter::SPIN),
            parameter::HIGH_FREQ | parameter::HIGH_RATIO => Some(parameter::HIGH),
            parameter::LOW_FREQ | parameter::LOW_RATIO => Some(parameter::LOW),
            parameter::CHORUS_AMOUNT | parameter::CHORUS_RATE => Some(parameter::CHORUS),
            _ => None,
        };
        Ok(gate.is_none_or(|gate| settings.on(gate)))
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess as A, ParameterAutomation as E, ParameterChangeImpact as I,
            ParameterModulation as M, ParameterNormalization as N, ParameterSemantics,
            ParameterSmoothing as S, ParameterStepping as T,
        };
        decode(state)?;
        let i = index(id)?;
        if let Some(semantics) = runtime::automation::semantics(id) {
            return Ok(Some(semantics));
        }
        let (normalization, stepping) = match DESCRIPTORS[i].kind {
            ParameterKind::Bool => (N::Discrete, T::Boolean),
            ParameterKind::Choice { .. } => (N::Discrete, T::Choice),
            ParameterKind::Float { .. } => (
                if DESCRIPTORS[i].presentation
                    == hex_lattice_contract::ParameterPresentation::Logarithmic
                {
                    N::PositiveLogarithmic
                } else {
                    N::Linear
                },
                T::Continuous,
            ),
        };
        Ok(Some(ParameterSemantics {
            access: A::ReadWrite,
            normalization,
            stepping,
            automation: if id == parameter::DRY_WET {
                E::DeviceArrangementEnvelope
            } else {
                E::Unsupported
            },
            modulation: M::Unsupported,
            smoothing: S::DeviceManaged,
            change_impact: I::ValueOnly,
        }))
    }
}
impl NativeDeviceDefinition for ReverbDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedReverb>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(runtime::automation::supports(parameter))
    }

    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        runtime::automation::validate_ids(parameters)?;
        Ok(hex_lattice::PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(hex_lattice::PreparedProcessorFactory::Legacy(
                std::sync::Arc::new(Self::prepare_owned_room_for_parameters(
                    state,
                    context.sample_rate,
                    parameters,
                )?),
            )),
        })
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
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        Ok(Some(Box::new(runtime::PreparedReverb::new(
            context.sample_rate,
            decode(state)?,
        )?)))
    }
}
#[cfg(test)]
mod tests;

/// Control-side static input response and nominal decay contour, not live audio.
#[derive(Clone, Debug, PartialEq)]
pub struct ReverbPreview {
    pub input: [[f32; 2]; 65],
    pub decay: [[f32; 2]; 65],
}
/// The input graph follows the provider's resonant TPT filter laws at the supplied
/// host rate. The decay graph shows authored low/mid/high RT60 ratios, not a
/// frequency response or an instantaneous spectrum of the recursive network.
pub fn preview(
    state: &NativeDeviceState,
    sample_rate: u32,
) -> Result<ReverbPreview, DeviceContractError> {
    if !(8000..=192000).contains(&sample_rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let settings = decode(&state::migrate(state)?)?;
    let rate = sample_rate as f32 / sample_rate.div_ceil(48000) as f32;
    // Bilinear transform of the same second-order low/high-pass used by the
    // runtime, evaluated without processing audio or constructing DSP history.
    let response = |hz: f32, cutoff: f32, q: f32, high: bool| {
        let g = (std::f32::consts::PI * cutoff.min(rate * 0.45) / rate).tan();
        let w = (std::f32::consts::PI * hz.min(rate * 0.49) / rate).tan() / g;
        let denominator = ((1.0 - w * w).powi(2) + (w / q).powi(2)).sqrt();
        (if high { w * w } else { 1.0 }) / denominator.max(1e-12)
    };
    let input = std::array::from_fn(|i| {
        let x = i as f32 / 64.0;
        let hz = 20.0 * 1000.0_f32.powf(x);
        let mut gain = 1.0;
        if settings.on(parameter::LO_CUT) {
            gain *= response(
                hz,
                settings.get(parameter::LO_FREQ),
                settings.get(parameter::LO_Q),
                true,
            );
        }
        if settings.on(parameter::HI_CUT) {
            gain *= response(
                hz,
                settings.get(parameter::HI_FREQ),
                settings.get(parameter::HI_Q),
                false,
            );
        }
        [
            x,
            ((48.0 + 20.0 * gain.max(1e-6).log10()) / 72.0).clamp(0.0, 1.0),
        ]
    });
    let decay = std::array::from_fn(|i| {
        let x = i as f32 / 64.0;
        let hz = 20.0 * 1000.0_f32.powf(x);
        let low = if settings.on(parameter::LOW) {
            settings.get(parameter::LOW_RATIO)
        } else {
            1.0
        };
        let high = if settings.on(parameter::HIGH) {
            settings.get(parameter::HIGH_RATIO)
        } else {
            1.0
        };
        let lo = 1.0 / (1.0 + (hz / settings.get(parameter::LOW_FREQ)).powi(2));
        let hi = 1.0 / (1.0 + (settings.get(parameter::HIGH_FREQ) / hz).powi(2));
        let ratio = if settings.on(parameter::FREEZE) && settings.on(parameter::FLAT) {
            1.0
        } else {
            low * lo + (1.0 - lo) * ((1.0 - hi) + high * hi)
        };
        [x, (ratio / 2.0).clamp(0.0, 1.0)]
    });
    Ok(ReverbPreview { input, decay })
}
