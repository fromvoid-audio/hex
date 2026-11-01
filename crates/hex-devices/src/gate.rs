//! Original zero-lookahead, linked-peak hysteresis gate.

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const GATE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.gate");
pub const GATE_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 24;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
/// Shortest internal hold, whatever the hold control says (Pulse #4107).
///
/// The detector is the instantaneous linked peak, so a low-frequency tone
/// dips below the return level around every zero crossing. With no hold the
/// gate closed there and chattered. Holding at least 5 ms rides through those
/// dips (a sine at amplitude `a` stays below return `r` for
/// `2 asin(r / a) / omega`, about 4.75 ms at 40 Hz just above a 5 dB
/// hysteresis), so the hold control's range below 5 ms acts as 5 ms. Lower
/// tones, or a return level close to the threshold, can still exceed it.
/// This is a hex decision, not a measured reference behaviour.
const MIN_HOLD_MS: f32 = 5.0;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const THRESHOLD: ParameterId = ParameterId::from_static(1);
    pub const RETURN: ParameterId = ParameterId::from_static(2);
    pub const FLOOR: ParameterId = ParameterId::from_static(3);
    pub const ATTACK: ParameterId = ParameterId::from_static(4);
    pub const HOLD: ParameterId = ParameterId::from_static(5);
    pub const RELEASE: ParameterId = ParameterId::from_static(6);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::THRESHOLD,
        label: "threshold",
        kind: ParameterKind::Float {
            min: -72.0,
            max: 0.0,
            unit: Some("dbfs"),
        },
        default: ParameterValue::Float(-40.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RETURN,
        label: "return",
        kind: ParameterKind::Float {
            min: -84.0,
            max: 0.0,
            unit: Some("dbfs"),
        },
        default: ParameterValue::Float(-45.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::FLOOR,
        label: "floor",
        kind: ParameterKind::Float {
            min: -80.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-60.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::ATTACK,
        label: "attack",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 100.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::HOLD,
        label: "hold",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 2_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(50.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 2_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct GateSettings {
    threshold_db: f32,
    return_db: f32,
    floor_db: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            return_db: -45.0,
            floor_db: -60.0,
            attack_ms: 1.0,
            hold_ms: 50.0,
            release_ms: 100.0,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<GateSettings, DeviceContractError> {
    if state.type_id() != GATE_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != GATE_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    if bytes.len() != STATE_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let read = |offset| {
        f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let settings = GateSettings {
        threshold_db: read(0),
        return_db: read(4),
        floor_db: read(8),
        attack_ms: read(12),
        hold_ms: read(16),
        release_ms: read(20),
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: GateSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::THRESHOLD, settings.threshold_db),
        (parameter::RETURN, settings.return_db),
        (parameter::FLOOR, settings.floor_db),
        (parameter::ATTACK, settings.attack_ms),
        (parameter::HOLD, settings.hold_ms),
        (parameter::RELEASE, settings.release_ms),
    ] {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(ParameterValue::Float(value))?;
    }
    if settings.return_db > settings.threshold_db {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}

fn encode(settings: GateSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.threshold_db,
        settings.return_db,
        settings.floor_db,
        settings.attack_ms,
        settings.hold_ms,
        settings.release_ms,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(GATE_TYPE_ID, GATE_STATE_VERSION, payload)
}

/// Registered Lattice Gate metadata, state codec, and control-side preparation.
pub struct GateDefinition;

pub static GATE_DEFINITION: GateDefinition = GateDefinition;

impl NativeDeviceMetadata for GateDefinition {
    fn type_id(&self) -> DeviceTypeId {
        GATE_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("gate")
    }
    fn state_version(&self) -> u32 {
        GATE_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(GateSettings::default())
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
        let value = match id {
            parameter::THRESHOLD => settings.threshold_db,
            parameter::RETURN => settings.return_db,
            parameter::FLOOR => settings.floor_db,
            parameter::ATTACK => settings.attack_ms,
            parameter::HOLD => settings.hold_ms,
            parameter::RELEASE => settings.release_ms,
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        Ok(ParameterValue::Float(value))
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
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        match id {
            parameter::THRESHOLD => settings.threshold_db = value,
            parameter::RETURN => settings.return_db = value,
            parameter::FLOOR => settings.floor_db = value,
            parameter::ATTACK => settings.attack_ms = value,
            parameter::HOLD => settings.hold_ms = value,
            parameter::RELEASE => settings.release_ms = value,
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        encode(settings)
    }
}

impl NativeDeviceDefinition for GateDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
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
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let attack_frames = checked_frames(settings.attack_ms, sample_rate)?;
        let hold_frames = checked_frames(settings.hold_ms.max(MIN_HOLD_MS), sample_rate)?;
        let release_frames = checked_frames(settings.release_ms, sample_rate)?;
        Ok(Some(Box::new(PreparedGate {
            settings,
            sample_rate,
            threshold: db_to_gain(settings.threshold_db)?,
            return_level: db_to_gain(settings.return_db)?,
            floor_gain: db_to_gain(settings.floor_db)?,
            attack_frames,
            hold_frames,
            release_frames,
        })))
    }
}

fn db_to_gain(db: f32) -> Result<f64, DeviceContractError> {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map(hex_dsp::LinearAmplitude::value)
        .map_err(|_| DeviceContractError::InvalidParameterValue)
}

fn checked_frames(milliseconds: f32, sample_rate: u32) -> Result<u32, DeviceContractError> {
    let frames = (f64::from(milliseconds) * f64::from(sample_rate) / 1_000.0).ceil();
    if !frames.is_finite() || frames < 1.0 || frames > f64::from(u32::MAX) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(frames as u32)
}

struct PreparedGate {
    settings: GateSettings,
    sample_rate: u32,
    threshold: f64,
    return_level: f64,
    floor_gain: f64,
    attack_frames: u32,
    hold_frames: u32,
    release_frames: u32,
}

impl PreparedNativeProcessor for PreparedGate {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<GateRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(GateRuntime {
            settings: self.settings,
            sample_rate: self.sample_rate,
            threshold: self.threshold,
            return_level: self.return_level,
            floor_gain: self.floor_gain,
            attack_frames: self.attack_frames,
            hold_frames: self.hold_frames,
            release_frames: self.release_frames,
            open: false,
            gain: self.floor_gain,
            hold_elapsed: 0,
            ramp_target: self.floor_gain,
            ramp_step: 0.0,
            ramp_remaining: 0,
        })
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GateHistory {
    open: bool,
    gain: f64,
    hold_elapsed: u32,
    ramp_target: f64,
    ramp_step: f64,
    ramp_remaining: u32,
}

struct GateRuntime {
    settings: GateSettings,
    sample_rate: u32,
    threshold: f64,
    return_level: f64,
    floor_gain: f64,
    attack_frames: u32,
    hold_frames: u32,
    release_frames: u32,
    open: bool,
    gain: f64,
    hold_elapsed: u32,
    ramp_target: f64,
    ramp_step: f64,
    ramp_remaining: u32,
}

impl GateRuntime {
    fn history(&self) -> GateHistory {
        GateHistory {
            open: self.open,
            gain: self.gain,
            hold_elapsed: self.hold_elapsed,
            ramp_target: self.ramp_target,
            ramp_step: self.ramp_step,
            ramp_remaining: self.ramp_remaining,
        }
    }

    fn set_history(&mut self, history: GateHistory) {
        self.open = history.open;
        self.gain = history.gain;
        self.hold_elapsed = history.hold_elapsed;
        self.ramp_target = history.ramp_target;
        self.ramp_step = history.ramp_step;
        self.ramp_remaining = history.ramp_remaining;
    }

    fn reset_history(&mut self) {
        self.open = false;
        self.gain = self.floor_gain;
        self.hold_elapsed = 0;
        self.ramp_target = self.floor_gain;
        self.ramp_step = 0.0;
        self.ramp_remaining = 0;
    }

    fn start_ramp(&mut self, target: f64, frames: u32) {
        self.ramp_target = target;
        if target == self.gain || frames == 0 {
            self.gain = target;
            self.ramp_step = 0.0;
            self.ramp_remaining = 0;
        } else {
            self.ramp_step = (target - self.gain) / f64::from(frames);
            self.ramp_remaining = frames;
        }
    }

    fn advance_ramp(&mut self) {
        if self.ramp_remaining == 0 {
            return;
        }
        if self.ramp_remaining == 1 {
            self.gain = self.ramp_target;
            self.ramp_step = 0.0;
            self.ramp_remaining = 0;
        } else {
            self.gain += self.ramp_step;
            self.ramp_remaining -= 1;
        }
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let left = f64::from(frame[0]);
        let right = f64::from(frame[1]);
        if !left.is_finite() || !right.is_finite() {
            return;
        }
        let peak = left.abs().max(right.abs());
        if peak >= self.threshold {
            if !self.open {
                self.open = true;
                self.hold_elapsed = 0;
                self.start_ramp(1.0, self.attack_frames);
            } else {
                self.hold_elapsed = 0;
            }
        } else if self.open {
            if peak <= self.return_level {
                // `hold_frames` is at least `MIN_HOLD_MS`, never zero.
                if self.hold_elapsed < self.hold_frames {
                    self.hold_elapsed += 1;
                } else {
                    self.open = false;
                    self.hold_elapsed = 0;
                    self.start_ramp(self.floor_gain, self.release_frames);
                }
            } else {
                self.hold_elapsed = 0;
            }
        }
        self.advance_ramp();
        frame[0] = (left * self.gain) as f32;
        frame[1] = (right * self.gain) as f32;
    }
}

impl NativeProcessorRuntime for GateRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            self.process_frame(frame);
        }
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            let original = *frame;
            self.process_frame(frame);
            *frame = original;
        }
    }
    fn reset(&mut self) {
        self.reset_history();
    }
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.sample_rate == previous.sample_rate)
        else {
            return;
        };
        let old_settings = previous.settings;
        self.set_history(previous.history());
        if self.settings == old_settings {
            return;
        }
        let attack_changed = self.settings.attack_ms != old_settings.attack_ms;
        let release_changed = self.settings.release_ms != old_settings.release_ms;
        let floor_changed = self.settings.floor_db != old_settings.floor_db;
        if self.open && attack_changed && self.ramp_remaining > 0 && self.ramp_target == 1.0 {
            self.start_ramp(1.0, self.attack_frames);
        } else if !self.open && (floor_changed || (release_changed && self.ramp_remaining > 0)) {
            self.start_ramp(self.floor_gain, self.release_frames);
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
