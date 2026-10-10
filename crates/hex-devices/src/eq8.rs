//! Original zero-latency eight-band parametric equalizer.
//!
//! Coefficients are prepared on the control side from the RBJ audio-EQ
//! cookbook equations. The callback owns only a fixed coefficient bank and
//! transposed-direct-form-II histories.
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any as StdAny;
use std::sync::Arc;

pub const EQ8_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.eq8");
pub const BAND_COUNT: usize = 8;
pub const EQ8_RUNTIME_BYTES: usize = std::mem::size_of::<Eq8Runtime>();
const STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 1 + 4 + 5 * BAND_COUNT * 14;
const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;
const MAX_GAIN_DB: f32 = 24.0;
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 18.0;
const HISTORY_FLOOR: f64 = 1.0e-30;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MIN_SAMPLE_RATE: u32 = 8_000;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const fn band(bank: usize, band: usize, field: u32) -> ParameterId {
        ParameterId::from_static((bank as u32) * 40 + (band as u32) * 5 + field + 1)
    }
    pub const fn enabled(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 0)
    }
    pub const fn kind(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 1)
    }
    pub const fn frequency(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 2)
    }
    pub const fn gain(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 3)
    }
    pub const fn q(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 4)
    }
    pub const MODE: ParameterId = ParameterId::from_static(201);
    pub const OUTPUT_GAIN: ParameterId = ParameterId::from_static(202);
}

const PARAMETER_LABELS: [&str; 5] = ["enabled", "type", "frequency", "gain", "q"];
const BAND_TYPES: &[&str] = &[
    "bell",
    "low shelf",
    "high shelf",
    "low cut 12",
    "low cut 48",
    "high cut 12",
    "high cut 48",
    "notch",
];
const CHANNEL_MODES: &[&str] = &["stereo", "left/right", "mid/side"];
const DEFAULT_FREQUENCIES: [f32; BAND_COUNT] = [
    40.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 6_000.0, 12_000.0,
];

const fn make_descriptors() -> [ParameterDescriptor; 202] {
    let mut descriptors = [ParameterDescriptor {
        id: ParameterId::from_static(1),
        label: "a enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }; 202];
    let mut bank = 0;
    while bank < 5 {
        let mut band = 0;
        while band < BAND_COUNT {
            let base = bank * 40 + band * 5;
            descriptors[base] = ParameterDescriptor {
                id: parameter::enabled(bank, band),
                label: PARAMETER_LABELS[0],
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 1] = ParameterDescriptor {
                id: parameter::kind(bank, band),
                label: PARAMETER_LABELS[1],
                kind: ParameterKind::Choice { labels: BAND_TYPES },
                default: ParameterValue::Choice(0),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 2] = ParameterDescriptor {
                id: parameter::frequency(bank, band),
                label: PARAMETER_LABELS[2],
                kind: ParameterKind::Float {
                    min: MIN_HZ,
                    max: MAX_HZ,
                    unit: Some("hz"),
                },
                default: ParameterValue::Float(DEFAULT_FREQUENCIES[band]),
                presentation: ParameterPresentation::Logarithmic,
            };
            descriptors[base + 3] = ParameterDescriptor {
                id: parameter::gain(bank, band),
                label: PARAMETER_LABELS[3],
                kind: ParameterKind::Float {
                    min: -MAX_GAIN_DB,
                    max: MAX_GAIN_DB,
                    unit: Some("db"),
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 4] = ParameterDescriptor {
                id: parameter::q(bank, band),
                label: PARAMETER_LABELS[4],
                kind: ParameterKind::Float {
                    min: MIN_Q,
                    max: MAX_Q,
                    unit: None,
                },
                default: ParameterValue::Float(std::f32::consts::FRAC_1_SQRT_2),
                presentation: ParameterPresentation::Linear,
            };
            band += 1;
        }
        bank += 1;
    }
    descriptors[200] = ParameterDescriptor {
        id: parameter::MODE,
        label: "channel mode",
        kind: ParameterKind::Choice {
            labels: CHANNEL_MODES,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    };
    descriptors[201] = ParameterDescriptor {
        id: parameter::OUTPUT_GAIN,
        label: "output gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 16.0,
            unit: Some("linear"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::LinearAmplitudeDecibels,
    };
    descriptors
}

static DESCRIPTORS: [ParameterDescriptor; 202] = make_descriptors();

mod state;
use state::{Band, ChannelMode, Settings, decode, encode};

pub struct Eq8Definition;

impl NativeDeviceMetadata for Eq8Definition {
    fn type_id(&self) -> DeviceTypeId {
        EQ8_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("eq eight")
    }
    fn state_version(&self) -> u32 {
        STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings::default())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        encode(decode(state)?)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = decode(state)?;
        if id == parameter::MODE {
            return Ok(ParameterValue::Choice(match settings.mode {
                ChannelMode::Stereo => 0,
                ChannelMode::LeftRight => 1,
                ChannelMode::MidSide => 2,
            }));
        }
        if id == parameter::OUTPUT_GAIN {
            return Ok(ParameterValue::Float(settings.output_gain));
        }
        let (bank, field) = decode_parameter_id(id)?;
        let band = settings.banks[bank][field / 5];
        Ok(match field % 5 {
            0 => ParameterValue::Bool(band.enabled),
            1 => ParameterValue::Choice(u32::from(band.kind)),
            2 => ParameterValue::Float(band.frequency_hz),
            3 => ParameterValue::Float(band.gain_db),
            4 => ParameterValue::Float(band.q),
            _ => return Err(DeviceContractError::UnknownParameter),
        })
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        if id == parameter::MODE {
            settings.mode = match value {
                ParameterValue::Choice(0) => ChannelMode::Stereo,
                ParameterValue::Choice(1) => ChannelMode::LeftRight,
                ParameterValue::Choice(2) => ChannelMode::MidSide,
                _ => return Err(DeviceContractError::InvalidParameterValue),
            };
            return encode(settings);
        }
        if id == parameter::OUTPUT_GAIN {
            let ParameterValue::Float(gain) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.output_gain = gain;
            return encode(settings);
        }
        let (bank, field) = decode_parameter_id(id)?;
        let band = &mut settings.banks[bank][field / 5];
        match (field % 5, value) {
            (0, ParameterValue::Bool(v)) => band.enabled = v,
            (1, ParameterValue::Choice(v)) => band.kind = v as u8,
            (2, ParameterValue::Float(v)) => band.frequency_hz = v,
            (3, ParameterValue::Float(v)) => band.gain_db = v,
            (4, ParameterValue::Float(v)) => band.q = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        encode(settings)
    }

    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let _ = decode(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        if id.value() <= 200 {
            let (bank, field) = decode_parameter_id(id)?;
            let kind = match self.read_parameter(state, parameter::kind(bank, field / 5))? {
                ParameterValue::Choice(kind) => kind,
                _ => return Err(DeviceContractError::MalformedState),
            };
            if field % 5 == 3 && matches!(kind, 3..=7) {
                return Ok(false);
            }
        }
        let value = self.read_parameter(state, id)?;
        descriptor.validate(value)?;
        Ok(true)
    }
}

fn decode_parameter_id(id: ParameterId) -> Result<(usize, usize), DeviceContractError> {
    let value = id.value();
    if !(1..=200).contains(&value) {
        return Err(DeviceContractError::UnknownParameter);
    }
    let zero = value - 1;
    Ok(((zero / 40) as usize, (zero % 40) as usize))
}

impl NativeDeviceDefinition for Eq8Definition {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
        Ok(StereoMatrix::identity())
    }

    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let settings = decode(state)?;
        let prepared = PreparedEq8::new(settings, sample_rate)?;
        Ok(Some(Box::new(prepared)))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

type BandStages = [Coefficients; 4];

#[derive(Clone)]
struct PreparedEq8 {
    sample_rate: u32,
    mode: ChannelMode,
    output_gain: f64,
    target: Arc<[[[Coefficients; 4]; BAND_COUNT]; 2]>,
}

impl PreparedEq8 {
    fn new(settings: Settings, sample_rate: u32) -> Result<Self, DeviceContractError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut banks = [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 5];
        for (bank, bank_stages) in banks.iter_mut().enumerate() {
            for (band_index, stages) in bank_stages.iter_mut().enumerate() {
                let band = settings.banks[bank][band_index];
                if band.enabled {
                    *stages = design_band(band, sample_rate)?;
                }
            }
        }
        let selected = match settings.mode {
            ChannelMode::Stereo => [0, 0],
            ChannelMode::LeftRight => [1, 2],
            ChannelMode::MidSide => [3, 4],
        };
        let mut target = [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 2];
        for channel in 0..2 {
            target[channel] = banks[selected[channel]];
        }
        Ok(Self {
            sample_rate,
            mode: settings.mode,
            output_gain: f64::from(settings.output_gain),
            target: Arc::new(target),
        })
    }

    fn runtime(&self) -> Eq8Runtime {
        Eq8Runtime {
            sample_rate: self.sample_rate,
            mode: self.mode,
            current_gain: self.output_gain,
            target_gain: self.output_gain,
            coefficients: *self.target,
            target: Arc::clone(&self.target),
            smooth_remaining: 0,
            active_bands: active_band_flags(&self.target),
            history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
        }
    }
}

impl PreparedNativeProcessor for PreparedEq8 {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<Eq8Runtime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(self.runtime())
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn frequency_response_at_hz(&self, frequency_hz: f32) -> Option<[f32; 2]> {
        if !frequency_hz.is_finite()
            || frequency_hz <= 0.0
            || frequency_hz >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        let response = std::array::from_fn(|channel| {
            let mut magnitude = self.output_gain;
            for band in 0..BAND_COUNT {
                for stage in 0..4 {
                    magnitude *= response_magnitude(
                        self.target[channel][band][stage],
                        f64::from(frequency_hz),
                        f64::from(self.sample_rate),
                    );
                }
            }
            finite_f32(magnitude)
        });
        Some(response)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

#[derive(Clone, Debug)]
pub struct Eq8Runtime {
    sample_rate: u32,
    mode: ChannelMode,
    current_gain: f64,
    target_gain: f64,
    coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    target: Arc<[[[Coefficients; 4]; BAND_COUNT]; 2]>,
    smooth_remaining: u16,
    active_bands: [[bool; BAND_COUNT]; 2],
    history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
}

const COEFFICIENT_SMOOTH_FRAMES: u16 = 256;

impl Eq8Runtime {
    pub fn process_frame(&mut self, input: StereoFrame) -> StereoFrame {
        self.advance_coefficients();
        let source = match self.mode {
            ChannelMode::Stereo | ChannelMode::LeftRight => {
                [f64::from(input[0]), f64::from(input[1])]
            }
            ChannelMode::MidSide => [
                (f64::from(input[0]) + f64::from(input[1])) * 0.5,
                (f64::from(input[0]) - f64::from(input[1])) * 0.5,
            ],
        };
        let mut filtered = source;
        for (channel, filtered_channel) in filtered.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                if !self.active_bands[channel][band] {
                    continue;
                }
                for stage in 0..4 {
                    let coefficients = self.coefficients[channel][band][stage];
                    let history = &mut self.history[channel][band][stage];
                    let output = coefficients.b0 * *filtered_channel + history[0];
                    history[0] = quiet(
                        coefficients.b1 * *filtered_channel - coefficients.a1 * output + history[1],
                    );
                    history[1] =
                        quiet(coefficients.b2 * *filtered_channel - coefficients.a2 * output);
                    *filtered_channel = quiet(output);
                }
                self.active_bands[channel][band] = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || self.target[channel][band][stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
            }
        }
        let output = match self.mode {
            ChannelMode::Stereo | ChannelMode::LeftRight => filtered,
            ChannelMode::MidSide => [filtered[0] + filtered[1], filtered[0] - filtered[1]],
        };
        [
            finite_f32(output[0] * self.current_gain),
            finite_f32(output[1] * self.current_gain),
        ]
    }

    pub fn process(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
    }

    pub fn reset(&mut self) {
        self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
        self.coefficients = *self.target;
        self.current_gain = self.target_gain;
        self.smooth_remaining = 0;
        self.active_bands = active_band_flags(&self.target);
    }

    pub fn inherit_history_from(&mut self, previous: &Self) -> bool {
        if self.sample_rate != previous.sample_rate {
            self.reset();
            false
        } else if self.mode != previous.mode {
            // The lane interpretation changes (stereo/LR/MS), so previous
            // histories cannot be reused safely. The configuration handoff is
            // still bounded and valid; begin the new routing with empty state.
            self.reset();
            false
        } else {
            self.current_gain = previous.current_gain;
            let same_target =
                *previous.target == *self.target && previous.target_gain == self.target_gain;
            self.smooth_remaining = if same_target {
                previous.smooth_remaining
            } else {
                COEFFICIENT_SMOOTH_FRAMES
            };
            self.coefficients = previous.coefficients;
            self.history = previous.history;
            self.refresh_active_bands();
            true
        }
    }

    fn advance_coefficients(&mut self) {
        if self.smooth_remaining == 0 {
            return;
        }
        let divisor = f64::from(self.smooth_remaining);
        for channel in 0..2 {
            for band in 0..BAND_COUNT {
                for stage in 0..4 {
                    let current = &mut self.coefficients[channel][band][stage];
                    let target = self.target[channel][band][stage];
                    current.b0 += (target.b0 - current.b0) / divisor;
                    current.b1 += (target.b1 - current.b1) / divisor;
                    current.b2 += (target.b2 - current.b2) / divisor;
                    current.a1 += (target.a1 - current.a1) / divisor;
                    current.a2 += (target.a2 - current.a2) / divisor;
                }
            }
        }
        self.smooth_remaining -= 1;
        if self.smooth_remaining == 0 {
            self.coefficients = *self.target;
            self.current_gain = self.target_gain;
            self.refresh_active_bands();
        } else {
            self.current_gain += (self.target_gain - self.current_gain) / divisor;
        }
    }

    fn refresh_active_bands(&mut self) {
        for channel in 0..2 {
            for band in 0..BAND_COUNT {
                self.active_bands[channel][band] = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || self.target[channel][band][stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
            }
        }
    }

    pub fn is_quiescent(&self) -> bool {
        self.history
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .all(|value| *value == 0.0)
    }
}

fn active_band_flags(
    coefficients: &[[[Coefficients; 4]; BAND_COUNT]; 2],
) -> [[bool; BAND_COUNT]; 2] {
    let mut active = [[false; BAND_COUNT]; 2];
    for channel in 0..2 {
        for band in 0..BAND_COUNT {
            active[channel][band] =
                (0..4).any(|stage| coefficients[channel][band][stage] != Coefficients::IDENTITY);
        }
    }
    active
}

impl NativeProcessorRuntime for Eq8Runtime {
    fn process_block(&mut self, frames: &mut [StereoFrame]) {
        self.process(frames);
    }
    fn reset(&mut self) {
        Eq8Runtime::reset(self);
    }
    fn is_quiescent(&self) -> bool {
        Eq8Runtime::is_quiescent(self)
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            let _ = self.inherit_history_from(previous);
        }
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

fn quiet(value: f64) -> f64 {
    if value.abs() < HISTORY_FLOOR {
        0.0
    } else {
        value
    }
}

fn finite_f32(value: f64) -> f32 {
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    }
}

fn response_magnitude(coefficients: Coefficients, frequency_hz: f64, sample_rate: f64) -> f64 {
    let angle = std::f64::consts::TAU * frequency_hz / sample_rate;
    let (sin, cos) = angle.sin_cos();
    let (sin2, cos2) = (2.0 * angle).sin_cos();
    let real_numerator = coefficients.b0 + coefficients.b1 * cos + coefficients.b2 * cos2;
    let imaginary_numerator = -coefficients.b1 * sin - coefficients.b2 * sin2;
    let real_denominator = 1.0 + coefficients.a1 * cos + coefficients.a2 * cos2;
    let imaginary_denominator = -coefficients.a1 * sin - coefficients.a2 * sin2;
    let numerator = real_numerator.hypot(imaginary_numerator);
    let denominator = real_denominator.hypot(imaginary_denominator);
    if denominator > f64::MIN_POSITIVE {
        numerator / denominator
    } else {
        f64::MAX
    }
}

fn design_band(band: Band, sample_rate: u32) -> Result<BandStages, DeviceContractError> {
    let nyquist = f64::from(sample_rate) * 0.5;
    let frequency = f64::from(band.frequency_hz).min(nyquist * 0.98);
    if frequency <= 0.0 || !frequency.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let omega = std::f64::consts::TAU * frequency / f64::from(sample_rate);
    let cosine = omega.cos();
    let sine = omega.sin();
    let q = f64::from(band.q);
    let mut stages = [Coefficients::IDENTITY; 4];
    match band.kind {
        0 => stages[0] = peaking(cosine, sine, q, f64::from(band.gain_db))?,
        1 => stages[0] = shelf(cosine, sine, q, f64::from(band.gain_db), true)?,
        2 => stages[0] = shelf(cosine, sine, q, f64::from(band.gain_db), false)?,
        3 => stages[0] = pass_filter(cosine, sine, q, true)?,
        4 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] = pass_filter(cosine, sine, section_q, true)?;
            }
        }
        5 => stages[0] = pass_filter(cosine, sine, q, false)?,
        6 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] = pass_filter(cosine, sine, section_q, false)?
            }
        }
        7 => stages[0] = notch(cosine, sine, q)?,
        _ => return Err(DeviceContractError::InvalidParameterValue),
    }
    Ok(stages)
}

fn normalized(
    b0: f64,
    b1: f64,
    b2: f64,
    a0: f64,
    a1: f64,
    a2: f64,
) -> Result<Coefficients, DeviceContractError> {
    if !a0.is_finite() || a0.abs() < f64::MIN_POSITIVE {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let result = Coefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    };
    if [result.b0, result.b1, result.b2, result.a1, result.a2]
        .iter()
        .all(|v| v.is_finite())
    {
        Ok(result)
    } else {
        Err(DeviceContractError::InvalidParameterValue)
    }
}

fn pass_filter(
    cosine: f64,
    sine: f64,
    q: f64,
    high_pass: bool,
) -> Result<Coefficients, DeviceContractError> {
    let alpha = sine / (2.0 * q);
    if high_pass {
        normalized(
            (1.0 + cosine) * 0.5,
            -(1.0 + cosine),
            (1.0 + cosine) * 0.5,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    } else {
        normalized(
            (1.0 - cosine) * 0.5,
            1.0 - cosine,
            (1.0 - cosine) * 0.5,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }
}

fn peaking(
    cosine: f64,
    sine: f64,
    q: f64,
    gain_db: f64,
) -> Result<Coefficients, DeviceContractError> {
    let a = 10.0_f64.powf(gain_db / 40.0);
    let alpha = sine / (2.0 * q);
    normalized(
        1.0 + alpha * a,
        -2.0 * cosine,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * cosine,
        1.0 - alpha / a,
    )
}

fn notch(cosine: f64, sine: f64, q: f64) -> Result<Coefficients, DeviceContractError> {
    let alpha = sine / (2.0 * q);
    normalized(
        1.0,
        -2.0 * cosine,
        1.0,
        1.0 + alpha,
        -2.0 * cosine,
        1.0 - alpha,
    )
}

fn shelf(
    cosine: f64,
    sine: f64,
    slope: f64,
    gain_db: f64,
    low: bool,
) -> Result<Coefficients, DeviceContractError> {
    let a = 10.0_f64.powf(gain_db / 40.0);
    let alpha = sine * 0.5 * (((a + 1.0 / a) * (1.0 / slope - 1.0)) + 2.0).sqrt();
    let two = 2.0 * a.sqrt() * alpha;
    let beta = 2.0 * a.sqrt() * alpha;
    let (b0, b1, b2, a0, a1, a2) = if low {
        (
            a * ((a + 1.0) - (a - 1.0) * cosine + two),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cosine),
            a * ((a + 1.0) - (a - 1.0) * cosine - two),
            (a + 1.0) + (a - 1.0) * cosine + beta,
            -2.0 * ((a - 1.0) + (a + 1.0) * cosine),
            (a + 1.0) + (a - 1.0) * cosine - beta,
        )
    } else {
        (
            a * ((a + 1.0) + (a - 1.0) * cosine + two),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cosine),
            a * ((a + 1.0) + (a - 1.0) * cosine - two),
            (a + 1.0) - (a - 1.0) * cosine + beta,
            2.0 * ((a - 1.0) - (a + 1.0) * cosine),
            (a + 1.0) - (a - 1.0) * cosine - beta,
        )
    };
    normalized(b0, b1, b2, a0, a1, a2)
}

pub static EQ8_DEFINITION: Eq8Definition = Eq8Definition;

#[cfg(test)]
#[path = "eq8/tests.rs"]
mod tests;
