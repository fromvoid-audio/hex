//! Original stereo cutoff filter. Version 1 stores thirteen 32-bit little-endian
//! words in descriptor order. The TPT kernel is shared with Operator. The
//! per-channel cutoff is modulated by a free-running oscillator and a linked
//! peak envelope. All authored values are checked before runtime construction.

mod runtime;

use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, PreparedNativeProcessor, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
pub use runtime::{FilterRuntime, PreparedFilter};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.filter");
const VERSION: u32 = 1;
const COUNT: usize = 13;
const WORD_BYTES: usize = size_of::<u32>();

const TYPE: usize = 0;
const SLOPE: usize = 1;
const CUTOFF: usize = 2;
const RESONANCE: usize = 3;
const MORPH: usize = 4;
const MIX: usize = 5;
const LFO_AMOUNT: usize = 6;
const LFO_RATE: usize = 7;
const LFO_SHAPE: usize = 8;
const LFO_PHASE: usize = 9;
const ENV_AMOUNT: usize = 10;
const ENV_RELEASE: usize = 11;
const ENV_ATTACK: usize = 12;

const fn choice(
    index: usize,
    label: &'static str,
    labels: &'static [&'static str],
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Choice { labels },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    }
}

const fn value(
    index: usize,
    label: &'static str,
    unit: &'static str,
    min: f32,
    max: f32,
    default: f32,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Float {
            min,
            max,
            unit: Some(unit),
        },
        default: ParameterValue::Float(default),
        presentation,
    }
}

use ParameterPresentation::{Linear, Logarithmic};

const DESCRIPTORS: [ParameterDescriptor; COUNT] = [
    choice(TYPE, "type", crate::operator_filter::MODES),
    choice(SLOPE, "slope", &["12 db", "24 db"]),
    value(CUTOFF, "cutoff", "hz", 20.0, 20_000.0, 1000.0, Logarithmic),
    value(RESONANCE, "resonance", "%", 0.0, 100.0, 0.0, Linear),
    value(MORPH, "morph", "%", 0.0, 100.0, 50.0, Linear),
    value(MIX, "dry/wet", "%", 0.0, 100.0, 100.0, Linear),
    value(LFO_AMOUNT, "lfo amount", "oct", 0.0, 4.0, 0.0, Linear),
    value(LFO_RATE, "lfo rate", "hz", 0.02, 30.0, 0.5, Logarithmic),
    choice(
        LFO_SHAPE,
        "lfo shape",
        &["sine", "triangle", "saw up", "saw down", "square"],
    ),
    value(LFO_PHASE, "lfo stereo phase", "°", 0.0, 360.0, 0.0, Linear),
    value(ENV_AMOUNT, "envelope amount", "oct", -4.0, 4.0, 0.0, Linear),
    value(
        ENV_RELEASE,
        "envelope release",
        "ms",
        5.0,
        2000.0,
        120.0,
        Logarithmic,
    ),
    value(
        ENV_ATTACK,
        "envelope attack",
        "ms",
        0.1,
        200.0,
        5.0,
        Logarithmic,
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings([u32; COUNT]);

impl Settings {
    fn default_values() -> Result<Self, DeviceContractError> {
        let mut settings = Self([0; COUNT]);
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            settings.write(index, descriptor.default)?;
        }
        Ok(settings)
    }

    fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if state.payload().len() != COUNT * WORD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut settings = Self([0; COUNT]);
        for (word, bytes) in settings
            .0
            .iter_mut()
            .zip(state.payload().chunks_exact(WORD_BYTES))
        {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            descriptor.validate(settings.read(index))?;
        }
        Ok(settings)
    }

    fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(
            TYPE_ID,
            VERSION,
            self.0.into_iter().flat_map(u32::to_le_bytes).collect(),
        )
    }

    fn float(self, index: usize) -> f64 {
        f64::from(f32::from_bits(self.0[index]))
    }

    fn read(self, index: usize) -> ParameterValue {
        if matches!(DESCRIPTORS[index].kind, ParameterKind::Choice { .. }) {
            ParameterValue::Choice(self.0[index])
        } else {
            ParameterValue::Float(f32::from_bits(self.0[index]))
        }
    }

    fn write(&mut self, index: usize, value: ParameterValue) -> Result<(), DeviceContractError> {
        DESCRIPTORS[index].validate(value)?;
        self.0[index] = match value {
            ParameterValue::Float(value) => value.to_bits(),
            ParameterValue::Choice(value) => value,
            ParameterValue::Bool(_) => return Err(DeviceContractError::InvalidParameterValue),
        };
        Ok(())
    }
}

fn parameter_index(id: ParameterId) -> Result<usize, DeviceContractError> {
    DESCRIPTORS
        .iter()
        .position(|descriptor| descriptor.id == id)
        .ok_or(DeviceContractError::UnknownParameter)
}

/// Built-in effect metadata and validated control-side construction.
pub struct FilterDefinition;

pub static DEFINITION: FilterDefinition = FilterDefinition;

impl FilterDefinition {
    /// Sealed inline preparation for track, source and bus effect chains.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedFilter, DeviceContractError> {
        if !parameters.is_empty() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        state.require_asset_free_processing()?;
        PreparedFilter::new(Settings::decode(state)?, rate)
    }
}

impl NativeDeviceMetadata for FilterDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("filter")
    }
    fn state_version(&self) -> u32 {
        VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::default_values()?.encode()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::decode(state)?.encode()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Ok(Settings::decode(state)?.read(parameter_index(id)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = Settings::decode(state)?;
        settings.write(parameter_index(id)?, value)?;
        settings.encode()
    }
}

impl NativeDeviceDefinition for FilterDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedFilter>())
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

/// Static wet response before LFO and envelope modulation. Coordinates are
/// normalized across 20 Hz to the project rate's supported upper cutoff and
/// -48 to +48 dB. The provider's prepared kernel owns these response values.
pub fn response(
    state: &NativeDeviceState,
    rate: u32,
) -> Result<[[f32; 2]; 129], DeviceContractError> {
    let prepared = FilterDefinition::prepare_owned(state, rate, &[])?;
    let upper = 20_000.0_f64.min(f64::from(rate) * 0.45);
    let mut frame = prepared.frame;
    frame.wet = 1.0;
    let compact = crate::operator_filter::Prepared { frame, rate };
    Ok(std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let frequency = 20.0 * (upper / 20.0).powf(x);
        let magnitude = compact.magnitude_at_hz(frequency as f32);
        let y = ((20.0 * magnitude.log10() + 48.0) / 96.0).clamp(0.0, 1.0);
        [x as f32, y]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::EMPTY_DEVICE_RESOURCES;

    fn parameter(index: usize) -> ParameterId {
        DESCRIPTORS[index].id
    }

    fn patch(state: &NativeDeviceState, index: usize, value: ParameterValue) -> NativeDeviceState {
        DEFINITION
            .patch_parameter(state, parameter(index), value)
            .unwrap()
    }

    fn signal(frequency: f64) -> Vec<[f32; 2]> {
        (0..8192)
            .map(|sample| {
                let sample =
                    (std::f64::consts::TAU * frequency * f64::from(sample) / 48_000.0).sin() as f32;
                [sample; 2]
            })
            .collect()
    }

    fn render(state: &NativeDeviceState, signal: &mut [[f32; 2]], block: usize) {
        let mut runtime = FilterDefinition::prepare_owned(state, 48_000, &[])
            .unwrap()
            .create_runtime();
        for block in signal.chunks_mut(block) {
            runtime.process_block(block, &EMPTY_DEVICE_RESOURCES);
        }
    }

    fn power(signal: &[[f32; 2]]) -> f32 {
        signal[4096..].iter().map(|frame| frame[0] * frame[0]).sum()
    }

    #[test]
    fn cutoff_type_and_slope_follow_the_audio_response_and_dry_is_transparent() {
        let low = DEFINITION.default_state().unwrap();
        let high = patch(&low, TYPE, ParameterValue::Choice(1));
        let steep = patch(&low, SLOPE, ParameterValue::Choice(1));
        let dry = patch(&low, MIX, ParameterValue::Float(0.0));
        let mut pass = signal(4000.0);
        let input = pass.clone();
        let mut stop = input.clone();
        let mut steep_stop = input.clone();
        let mut dry_output = input.clone();
        render(&high, &mut pass, 127);
        render(&low, &mut stop, 127);
        render(&steep, &mut steep_stop, 127);
        render(&dry, &mut dry_output, 127);
        assert!(power(&pass) > 0.95 * power(&input));
        assert!(power(&stop) < power(&input) * 0.005);
        assert!(power(&steep_stop) < power(&stop) * 0.005);
        assert_eq!(dry_output, input);
        assert!(response(&low, 48_000).unwrap()[128][1] < response(&high, 48_000).unwrap()[128][1]);
    }

    #[test]
    fn cutoff_modulation_respects_polarity_stereo_phase_and_block_partition() {
        let state = DEFINITION.default_state().unwrap();
        let positive = patch(&state, ENV_AMOUNT, ParameterValue::Float(3.0));
        let negative = patch(&state, ENV_AMOUNT, ParameterValue::Float(-3.0));
        let lfo = patch(&state, LFO_AMOUNT, ParameterValue::Float(2.0));
        let lfo = patch(&lfo, LFO_PHASE, ParameterValue::Float(180.0));
        let input = signal(2000.0);
        let mut closed = input.clone();
        let mut open = input.clone();
        let mut whole = input.clone();
        let mut partitioned = input.clone();
        render(&negative, &mut closed, 256);
        render(&positive, &mut open, 256);
        render(&lfo, &mut whole, 8192);
        render(&lfo, &mut partitioned, 113);
        assert!(power(&open) > power(&closed) * 10.0);
        assert_eq!(whole, partitioned);
        assert!(whole.iter().any(|frame| (frame[0] - frame[1]).abs() > 0.1));
    }

    #[test]
    fn codec_round_trips_and_refuses_unvalidated_values_and_formats() {
        let original = DEFINITION.default_state().unwrap();
        let changed = patch(&original, CUTOFF, ParameterValue::Float(100.0));
        assert_eq!(DEFINITION.migrate_state(&changed).unwrap(), changed);
        assert_eq!(
            DEFINITION.read_parameter(&changed, parameter(CUTOFF)),
            Ok(ParameterValue::Float(100.0))
        );
        assert!(
            DEFINITION
                .patch_parameter(&changed, parameter(CUTOFF), ParameterValue::Float(f32::NAN))
                .is_err()
        );
        for version in [VERSION + 1, VERSION] {
            let broken = NativeDeviceState::new(TYPE_ID, version, vec![0; 4]).unwrap();
            assert!(DEFINITION.migrate_state(&broken).is_err());
        }
    }
}
