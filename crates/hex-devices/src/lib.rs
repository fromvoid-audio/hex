//! Original native DSP: registered instruments and native Lattice effects.
pub mod auto_pan;
pub mod compressor;
pub mod eq8;
pub mod fm4;
pub mod gate;
pub mod operator_envelope;
pub mod operator_state;
pub mod operator_waveform;
pub mod saturator;
pub mod utility;
pub mod utility_filters;

use hex_lattice::{
    NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use std::sync::OnceLock;

static BUILTIN_NATIVE_DEVICES: [&dyn hex_lattice::NativeDeviceDefinition; 6] = [
    &utility::UTILITY_DEFINITION,
    &eq8::EQ8_DEFINITION,
    &saturator::SATURATOR_DEFINITION,
    &compressor::COMPRESSOR_DEFINITION,
    &gate::GATE_DEFINITION,
    &auto_pan::AUTO_PAN_DEFINITION,
];
static BUILTIN_NATIVE_INSTRUMENTS: [&dyn NativeInstrumentDefinition; 3] = [
    &SINE_INSTRUMENT,
    &fm4::device::FM4_DEFINITION,
    &operator_state::OPERATOR_METADATA,
];

pub const SINE_INSTRUMENT_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.sine");

/// The immutable catalog of original built-in native effects.
pub fn native_registry()
-> Result<hex_lattice::NativeDeviceRegistry<'static>, hex_lattice_contract::DeviceContractError> {
    hex_lattice::NativeDeviceRegistry::new(&BUILTIN_NATIVE_DEVICES)
}

/// The immutable catalog of original analytic native instruments.
pub fn native_instrument_registry()
-> Result<hex_lattice::NativeInstrumentRegistry<'static>, DeviceContractError> {
    hex_lattice::NativeInstrumentRegistry::new(&BUILTIN_NATIVE_INSTRUMENTS)
}

/// Prepare the original sine configuration for callers that still use the
/// narrow legacy MIDI-template helper. New runtime compilation resolves the
/// track's generic instrument state through `native_instrument_registry`.
pub fn prepare_legacy_sine_instrument(
    sample_rate: u32,
) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
    let registry = native_instrument_registry()?;
    let state = registry.default_state(SINE_INSTRUMENT_TYPE_ID)?;
    registry.prepare_instrument(&state, sample_rate)
}

static SINE_INSTRUMENT: SineInstrumentDefinition = SineInstrumentDefinition;
struct SineInstrumentDefinition;
struct PreparedSineInstrument {
    sample_rate: u32,
}

impl NativeDeviceMetadata for SineInstrumentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SINE_INSTRUMENT_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("sine")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.type_id(), 1, Vec::new())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != self.type_id()
            || state.state_version() != 1
            || !state.payload().is_empty()
        {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl NativeInstrumentDefinition for SineInstrumentDefinition {
    fn prepare_instrument(
        &self,
        _state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        // Materialize the immutable lookup table on the control side before
        // any callback can invoke the sampler's function pointer.
        let _ = shared_sine_table();
        Ok(Box::new(PreparedSineInstrument { sample_rate }))
    }
}

impl PreparedNativeInstrument for PreparedSineInstrument {
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        let voice = shared_sine_table()
            .prepare_voice(self.sample_rate, pitch, velocity)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = voice.phase_step;
        payload[1] = u64::from(voice.amplitude.to_bits());
        payload[2] = voice.attack_frames;
        Ok(PreparedAnalyticNote::new(
            payload,
            voice.release_frames,
            sample_sine_note,
        ))
    }
}

fn sample_sine_note(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age_frames: u64,
    gate_frames: u64,
    release_frames: u64,
) -> StereoFrame {
    let voice = PreparedVoice {
        phase_step: payload[0],
        amplitude: f32::from_bits(payload[1] as u32),
        attack_frames: payload[2],
        release_frames,
    };
    let Some(table) = shared_sine_table_if_prepared() else {
        return [0.0; 2];
    };
    let sample = table.sample(voice, age_frames, gate_frames);
    [sample, sample]
}

static SHARED_SINE_TABLE: OnceLock<SineTable> = OnceLock::new();

fn shared_sine_table_if_prepared() -> Option<&'static SineTable> {
    SHARED_SINE_TABLE.get()
}

fn shared_sine_table() -> &'static SineTable {
    SHARED_SINE_TABLE.get_or_init(SineTable::new)
}

use std::fmt;

const TABLE_BITS: u32 = 11;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const TABLE_MASK: u64 = TABLE_SIZE as u64 - 1;
const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;
const BASE_GAIN: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Invalid {}

/// One immutable sine table shared by every compiled voice in a project.
#[derive(Debug)]
pub struct SineTable {
    samples: [f32; TABLE_SIZE + 1],
}
impl SineTable {
    pub fn new() -> Self {
        let mut samples = [0.0; TABLE_SIZE + 1];
        for (index, sample) in samples.iter_mut().take(TABLE_SIZE).enumerate() {
            *sample = (std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64).sin() as f32;
        }
        samples[TABLE_SIZE] = samples[0];
        Self { samples }
    }

    pub fn prepare_voice(
        &self,
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedVoice, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate)
            || pitch > 127
            || !(1..=127).contains(&velocity)
        {
            return Err(Invalid("invalid instrument sample rate or velocity"));
        }
        let frequency = 440.0_f64 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
        if !frequency.is_finite() || frequency >= f64::from(sample_rate) * 0.5 {
            return Err(Invalid("instrument pitch must be below nyquist"));
        }
        let phase_per_frame = frequency / f64::from(sample_rate);
        let phase_step = (phase_per_frame * PHASE_SCALE).round();
        if !phase_per_frame.is_finite() || !phase_step.is_finite() || phase_step < 1.0 {
            return Err(Invalid("instrument phase step is invalid"));
        }
        let attack_frames = (u64::from(sample_rate) * 5 / 1000).max(1);
        let release_frames = (u64::from(sample_rate) * 50 / 1000).max(1);
        Ok(PreparedVoice {
            phase_step: phase_step as u64,
            amplitude: f32::from(velocity) / 127.0 * BASE_GAIN,
            attack_frames,
            release_frames,
        })
    }

    /// Produce one sample from a voice whose note began at relative frame zero.
    /// `gate_frames` marks note-off; output is zero at the exclusive release end.
    pub fn sample(&self, voice: PreparedVoice, relative_frame: u64, gate_frames: u64) -> f32 {
        let release_end = gate_frames.saturating_add(voice.release_frames);
        if relative_frame >= release_end {
            return 0.0;
        }
        let amplitude = if relative_frame < gate_frames {
            (relative_frame as f32 / voice.attack_frames as f32).min(1.0)
        } else {
            let attack_at_gate = (gate_frames as f32 / voice.attack_frames as f32).min(1.0);
            let release_progress =
                (relative_frame - gate_frames) as f32 / voice.release_frames as f32;
            attack_at_gate * (1.0 - release_progress).max(0.0)
        };
        let phase = voice.phase_step.wrapping_mul(relative_frame);
        self.sample_phase(phase) * amplitude * voice.amplitude
    }

    /// Interpolated periodic lookup; the caller supplies a wrapping turn phase.
    pub(crate) fn sample_phase(&self, phase: u64) -> f32 {
        let index = ((phase >> (64 - TABLE_BITS)) & TABLE_MASK) as usize;
        let fraction = ((phase << TABLE_BITS) >> 40) as u32 as f32 / 16_777_216.0;
        self.samples[index] + (self.samples[index + 1] - self.samples[index]) * fraction
    }
}
impl Default for SineTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PreparedVoice {
    phase_step: u64,
    amplitude: f32,
    attack_frames: u64,
    release_frames: u64,
}
impl PreparedVoice {
    pub fn release_frames(self) -> u64 {
        self.release_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_tone_has_expected_frequency_and_fixed_gain() {
        let table = SineTable::new();
        let voice = table.prepare_voice(48_000, 69, 127).unwrap();
        let samples: Vec<_> = (0..48_000)
            .map(|frame| table.sample(voice, frame, 48_000))
            .collect();
        assert!(samples.iter().any(|sample| sample.abs() > 0.09));
        let positive_crossings = samples
            .windows(2)
            .filter(|window| window[0] <= 0.0 && window[1] > 0.0)
            .count();
        assert_eq!(positive_crossings, 440);
    }

    #[test]
    fn attack_short_gate_release_and_exclusive_end_are_defined() {
        let table = SineTable::new();
        let voice = table.prepare_voice(8_000, 69, 127).unwrap();
        assert_eq!(table.sample(voice, 0, 5), 0.0);
        let expected = (std::f64::consts::TAU * 440.0 / 8_000.0).sin() as f32 * (0.1 / 40.0);
        assert!((table.sample(voice, 1, 5) - expected).abs() < 0.000_001);
        assert!(table.sample(voice, 5, 5).abs() > 0.0);
        assert!(table.sample(voice, 404, 5).abs() > 0.0);
        assert_eq!(table.sample(voice, 405, 5), 0.0);
    }

    #[test]
    fn rejects_nyquist_and_invalid_velocity() {
        let table = SineTable::new();
        assert!(matches!(
            table.prepare_voice(8_000, 127, 100),
            Err(Invalid("instrument pitch must be below nyquist"))
        ));
        assert!(table.prepare_voice(48_000, 128, 100).is_err());
        assert!(table.prepare_voice(1_000, 60, 100).is_err());
        assert!(table.prepare_voice(48_000, 60, 0).is_err());
    }

    #[test]
    fn widened_analytic_payload_preserves_legacy_sine_samples_exactly() {
        let table = SineTable::new();
        let legacy = table.prepare_voice(48_000, 69, 100).unwrap();
        let registry = native_instrument_registry().unwrap();
        let state = registry.default_state(SINE_INSTRUMENT_TYPE_ID).unwrap();
        let prepared = registry.prepare_instrument(&state, 48_000).unwrap();
        let note = prepared.prepare_note(69, 100).unwrap();
        assert_eq!(std::mem::size_of_val(&note), 344);
        for gate in [1, 24_000, 48_000] {
            for age in [
                0,
                1,
                239,
                240,
                gate - 1,
                gate,
                gate + 1,
                gate + 2_399,
                gate + 2_400,
            ] {
                let expected = table.sample(legacy, age, gate);
                assert_eq!(note.sample(age, gate), [expected, expected]);
            }
        }
    }
}

// Isolated harmonic-kernel proof pending canonical/host dependency integration.
mod operator_filter;
#[cfg(test)]
mod operator_harmonics;
