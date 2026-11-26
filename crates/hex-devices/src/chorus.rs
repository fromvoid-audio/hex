//! Original bounded stereo Chorus modulation effect for the native Lattice host.
//!
//! Architecture & bounds:
//! - Zero algorithmic latency (`0` frames) and bounded finite tail (`4 s` at the
//!   prepared sample rate with explicit silent-frame drain flush to strict
//!   quiescence).
//! - Twin preallocated delay rings (`committed` + `scratch`, `<= 45 ms + 8`
//!   frames per channel, allocated once on control via `try_reserve_exact`,
//!   zero callback allocations, `O(dirty_len)` commit/rollback, `O(1)` reset).
//! - Dual quadrature-modulated voices per channel with 4-point Catmull-Rom cubic
//!   fractional delay interpolation, wet-path 1-pole high-pass (`20..2000 Hz`)
//!   and analog warmth low-pass damping (`0..1`), bounded bipolar feedback
//!   (`-0.75..=0.75`), 5 ms `LinearRamp` parameter transitions, and full
//!   sample-rate automation across all 8 parameters.
//! - Non-finite input samples pass through untouched without polluting delay
//!   rings or filter histories.
//!
//! Saved state format:
//! - v1 (`32` bytes): 8 little-endian `f32` scalars (`rate_hz`, `amount`,
//!   `delay_ms`, `feedback`, `stereo_width`, `hpf_hz`, `warmth`, `dry_wet`).
//!
//! This is an original bounded Chorus implementation; no proprietary assets or
//! full chorus/flanger/phaser family parity are claimed.

use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, NativeInstrumentClock,
    NativeParameterAutomation, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, PreparedRuntimeDevice, ProcessorInstanceIdentity, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::f64::consts::TAU;
use std::sync::Arc;

pub const CHORUS_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.chorus");
pub const CHORUS_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 32;
const PARAM_COUNT: usize = 8;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const RAMP_MS: f64 = 5.0;
const MAX_MOD_EXCURSION_MS: f64 = 12.0;
const RING_DURATION_SECONDS: f64 = 0.045;
const RING_GUARD_FRAMES: usize = 8;
const TAIL_SECONDS: u32 = 4;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const RATE: ParameterId = ParameterId::from_static(1);
    pub const AMOUNT: ParameterId = ParameterId::from_static(2);
    pub const DELAY_MS: ParameterId = ParameterId::from_static(3);
    pub const FEEDBACK: ParameterId = ParameterId::from_static(4);
    pub const STEREO_WIDTH: ParameterId = ParameterId::from_static(5);
    pub const HPF_HZ: ParameterId = ParameterId::from_static(6);
    pub const WARMTH: ParameterId = ParameterId::from_static(7);
    pub const DRY_WET: ParameterId = ParameterId::from_static(8);
}

const SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };

const LOG_SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
        ..SMOOTHED_ENVELOPE
    };

const BINDINGS: [crate::parameter_binding::Binding<ChorusSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::RATE,
            label: "rate",
            kind: ParameterKind::Float {
                min: 0.05,
                max: 10.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(0.8),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.rate_hz,
        |c, v| c.rate_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::AMOUNT,
            label: "amount",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.45),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.amount,
        |c, v| c.amount = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DELAY_MS,
            label: "delay",
            kind: ParameterKind::Float {
                min: 1.0,
                max: 25.0,
                unit: Some("ms"),
            },
            default: ParameterValue::Float(7.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.delay_ms,
        |c, v| c.delay_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::FEEDBACK,
            label: "feedback",
            kind: ParameterKind::Float {
                min: -0.75,
                max: 0.75,
                unit: None,
            },
            default: ParameterValue::Float(0.15),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.feedback,
        |c, v| c.feedback = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::STEREO_WIDTH,
            label: "width",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.stereo_width,
        |c, v| c.stereo_width = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::HPF_HZ,
            label: "hpf",
            kind: ParameterKind::Float {
                min: 20.0,
                max: 2_000.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(80.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.hpf_hz,
        |c, v| c.hpf_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WARMTH,
            label: "warmth",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.2),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.warmth,
        |c, v| c.warmth = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DRY_WET,
            label: "dry/wet",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.dry_wet,
        |c, v| c.dry_wet = v,
    ),
];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChorusSettings {
    rate_hz: f32,
    amount: f32,
    delay_ms: f32,
    feedback: f32,
    stereo_width: f32,
    hpf_hz: f32,
    warmth: f32,
    dry_wet: f32,
}

impl Default for ChorusSettings {
    fn default() -> Self {
        Self {
            rate_hz: 0.8,
            amount: 0.45,
            delay_ms: 7.0,
            feedback: 0.15,
            stereo_width: 1.0,
            hpf_hz: 80.0,
            warmth: 0.2,
            dry_wet: 0.5,
        }
    }
}

fn validate(settings: ChorusSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn decode(state: &NativeDeviceState) -> Result<ChorusSettings, DeviceContractError> {
    if state.type_id() != CHORUS_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != CHORUS_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    if bytes.len() != STATE_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let read = |offset: usize| -> Result<f32, DeviceContractError> {
        let slice: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(f32::from_le_bytes(slice))
    };
    let settings = ChorusSettings {
        rate_hz: read(0)?,
        amount: read(4)?,
        delay_ms: read(8)?,
        feedback: read(12)?,
        stereo_width: read(16)?,
        hpf_hz: read(20)?,
        warmth: read(24)?,
        dry_wet: read(28)?,
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: ChorusSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.rate_hz,
        settings.amount,
        settings.delay_ms,
        settings.feedback,
        settings.stereo_width,
        settings.hpf_hz,
        settings.warmth,
        settings.dry_wet,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    NativeDeviceState::new(CHORUS_TYPE_ID, CHORUS_STATE_VERSION, payload)
}

fn parameter_slot(id: ParameterId) -> Option<usize> {
    id.value()
        .checked_sub(1)
        .map(|i| i as usize)
        .filter(|&i| i < PARAM_COUNT)
}

/// Registered native stereo Chorus definition.
pub struct ChorusDefinition;
pub static CHORUS_DEFINITION: ChorusDefinition = ChorusDefinition;

impl ChorusDefinition {
    /// Prepare the owned factory without allocating delay rings on control.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedChorus, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut admitted = [false; PARAM_COUNT];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            let slot = parameter_slot(id).ok_or(DeviceContractError::UnknownParameter)?;
            admitted[slot] = true;
        }
        Ok(PreparedChorus {
            settings,
            rate: sample_rate,
            admitted,
        })
    }
}

impl NativeDeviceMetadata for ChorusDefinition {
    fn type_id(&self) -> DeviceTypeId {
        CHORUS_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("chorus")
    }
    fn state_version(&self) -> u32 {
        CHORUS_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(&BINDINGS, state, id, decode)
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(ChorusSettings::default())
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
        Ok(crate::parameter_binding::binding(&BINDINGS, id)?.read(&decode(state)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(&BINDINGS, state, id, value, decode, encode)
    }
}

impl NativeDeviceDefinition for ChorusDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedChorus>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(parameter_slot(id).is_some())
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        let factory = Self::prepare_owned(state, context.sample_rate, parameters)?;
        Ok(PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(PreparedProcessorFactory::Owned(Arc::new(factory))),
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
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        ))))
    }
}

/// Control-prepared Chorus factory with bounded inline parameters.
#[derive(Clone, Copy, Debug)]
pub struct PreparedChorus {
    settings: ChorusSettings,
    rate: u32,
    admitted: [bool; PARAM_COUNT],
}

impl PreparedChorus {
    fn ring_len(&self) -> usize {
        ((f64::from(self.rate) * RING_DURATION_SECONDS).ceil() as usize) + RING_GUARD_FRAMES
    }

    fn tail_limit(&self) -> u32 {
        self.rate.saturating_mul(TAIL_SECONDS)
    }
}

impl PreparedOwnedProcessorRuntime for PreparedChorus {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        parameter_slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.supports_parameter_automation(id)
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, id)
                    .ok()
                    .map(|b| b.descriptor)
            })
            .flatten()
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        self.supports_parameter_automation(id)
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, id)
                    .ok()
                    .map(|b| b.semantics)
            })
            .flatten()
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<ChorusRuntime>()
            + self.ring_len() * 2 * std::mem::size_of::<StereoFrame>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let length = self.ring_len();
        let alloc_ring = || -> Result<Vec<StereoFrame>, DeviceContractError> {
            let mut ring = Vec::new();
            ring.try_reserve_exact(length)
                .map_err(|_| DeviceContractError::RuntimeAllocation)?;
            ring.resize(length, [0.0; 2]);
            Ok(ring)
        };
        let scalar = ChorusScalar::new(self.settings, self.rate, self.admitted, self.tail_limit());
        let runtime = ChorusRuntime {
            settings: self.settings,
            rate: self.rate,
            tail_limit: self.tail_limit(),
            committed: alloc_ring()?,
            scratch: alloc_ring()?,
            primary: scalar,
            speculative: scalar,
            dirty_start: 0,
            dirty_len: 0,
            open: false,
            pristine: true,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.tail_limit()),
        }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}

#[derive(Clone, Copy, Debug)]
struct ChorusScalar {
    controls: crate::parameter_binding::Controls<ChorusSettings, PARAM_COUNT>,
    next: usize,
    valid: usize,
    stale_slots: bool,
    phase: f64,
    ramps: [hex_dsp::LinearRamp; PARAM_COUNT],
    targets: ChorusSettings,
    hpf_state: [f64; 2],
    warmth_state: [f64; 2],
    silent_frames: u32,
}

impl ChorusScalar {
    fn new(
        settings: ChorusSettings,
        _rate: u32,
        admitted: [bool; PARAM_COUNT],
        tail_limit: u32,
    ) -> Self {
        Self {
            controls: crate::parameter_binding::Controls::new(settings, admitted),
            next: 0,
            valid: 0,
            stale_slots: false,
            phase: 0.0,
            ramps: [
                hex_dsp::LinearRamp::settled(f64::from(settings.rate_hz)),
                hex_dsp::LinearRamp::settled(f64::from(settings.amount)),
                hex_dsp::LinearRamp::settled(f64::from(settings.delay_ms)),
                hex_dsp::LinearRamp::settled(f64::from(settings.feedback)),
                hex_dsp::LinearRamp::settled(f64::from(settings.stereo_width)),
                hex_dsp::LinearRamp::settled(f64::from(settings.hpf_hz)),
                hex_dsp::LinearRamp::settled(f64::from(settings.warmth)),
                hex_dsp::LinearRamp::settled(f64::from(settings.dry_wet)),
            ],
            targets: settings,
            hpf_state: [0.0; 2],
            warmth_state: [0.0; 2],
            silent_frames: tail_limit,
        }
    }

    fn is_quiescent(&self, tail_limit: u32) -> bool {
        (self.valid == 0 || self.silent_frames >= tail_limit)
            && self.hpf_state == [0.0; 2]
            && self.warmth_state == [0.0; 2]
    }

    fn retarget(&mut self, next: ChorusSettings, ramp_frames: u32) {
        let old = [
            self.targets.rate_hz,
            self.targets.amount,
            self.targets.delay_ms,
            self.targets.feedback,
            self.targets.stereo_width,
            self.targets.hpf_hz,
            self.targets.warmth,
            self.targets.dry_wet,
        ];
        let target = [
            next.rate_hz,
            next.amount,
            next.delay_ms,
            next.feedback,
            next.stereo_width,
            next.hpf_hz,
            next.warmth,
            next.dry_wet,
        ];
        for i in 0..PARAM_COUNT {
            if target[i] != old[i] {
                self.ramps[i].retarget(f64::from(target[i]), ramp_frames);
            }
        }
        self.targets = next;
    }
}

/// Bounded transactional stereo Chorus runtime owning fixed preallocated rings.
pub struct ChorusRuntime {
    settings: ChorusSettings,
    rate: u32,
    tail_limit: u32,
    committed: Vec<StereoFrame>,
    scratch: Vec<StereoFrame>,
    primary: ChorusScalar,
    speculative: ChorusScalar,
    dirty_start: usize,
    dirty_len: usize,
    open: bool,
    pristine: bool,
}

#[inline]
fn catmull_rom(y0: f64, y1: f64, y2: f64, y3: f64, t: f64) -> f64 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

#[inline]
fn read_ring_cubic(
    ring: &[StereoFrame],
    write_pos: usize,
    valid: usize,
    delay_frames: f64,
    channel: usize,
) -> f64 {
    let len = ring.len();
    let max_d = (len.saturating_sub(4)) as f64;
    let d = delay_frames.clamp(1.0, max_d);
    let base = d.floor() as usize;
    let frac = d - base as f64;
    let sample_at = |lag: usize| -> f64 {
        if lag == 0 || lag > valid {
            return 0.0;
        }
        let idx = (write_pos + len - (lag % len)) % len;
        f64::from(ring[idx][channel])
    };
    let y0 = sample_at(base.saturating_sub(1));
    let y1 = sample_at(base);
    let y2 = sample_at(base + 1);
    let y3 = sample_at(base + 2);
    catmull_rom(y0, y1, y2, y3, frac)
}

impl ChorusRuntime {
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        let bytes = |ring: &Vec<StereoFrame>| {
            ring.capacity()
                .checked_mul(std::mem::size_of::<StereoFrame>())
        };
        Some((
            std::mem::size_of::<Self>(),
            [bytes(&self.committed)?, bytes(&self.scratch)?],
        ))
    }

    fn copy_interval(src: &[StereoFrame], dst: &mut [StereoFrame], start: usize, len: usize) {
        let n = len.min(src.len());
        if n == 0 {
            return;
        }
        let first = n.min(src.len() - start);
        dst[start..start + first].copy_from_slice(&src[start..start + first]);
        if first < n {
            let rest = n - first;
            dst[..rest].copy_from_slice(&src[..rest]);
        }
    }

    fn ramp_frames(&self) -> u32 {
        hex_dsp::ramp_frames_for_ms(self.rate, RAMP_MS).unwrap_or(1)
    }

    fn process_frame(&mut self, frame: &mut StereoFrame, bypassed: bool) {
        let s = &mut self.speculative;
        let rate_hz = s.ramps[0].advance();
        let amount = s.ramps[1].advance();
        let delay_ms = s.ramps[2].advance();
        let feedback = s.ramps[3].advance().clamp(-0.75, 0.75);
        let stereo_width = s.ramps[4].advance();
        let hpf_hz = s.ramps[5].advance();
        let warmth = s.ramps[6].advance();
        let dry_wet = s.ramps[7].advance();

        let sr = f64::from(self.rate);
        let phase = s.phase;
        s.phase = (s.phase + rate_hz / sr).rem_euclid(1.0);

        let in_l = f64::from(frame[0]);
        let in_r = f64::from(frame[1]);
        if !in_l.is_finite() || !in_r.is_finite() {
            return;
        }

        let silent = frame[0].to_bits() == 0 && frame[1].to_bits() == 0;
        if silent {
            s.silent_frames = s.silent_frames.saturating_add(1).min(self.tail_limit);
        } else {
            s.silent_frames = 0;
        }

        if silent && s.is_quiescent(self.tail_limit) {
            return;
        }

        let len = self.scratch.len();
        let max_delay_frames = (len.saturating_sub(4)) as f64;
        let mod_depth_ms = amount * MAX_MOD_EXCURSION_MS * 0.5;
        let hpf_pole = hex_dsp::frequency_pole(hpf_hz.clamp(20.0, sr * 0.45), sr).unwrap_or(0.0);
        // Warmth 0 -> open (pole 0.0); warmth 1 -> gentle ~3.5 kHz low-pass damping.
        let lpf_pole = if warmth <= 0.0 {
            0.0
        } else {
            let cutoff = (20_000.0 * (3_500.0_f64 / 20_000.0).powf(warmth)).min(sr * 0.45);
            hex_dsp::frequency_pole(cutoff, sr).unwrap_or(0.0)
        };

        let mut wet_out = [0.0_f64; 2];
        let mut to_write = [0.0_f32; 2];
        let inputs = [in_l, in_r];
        for ch in 0..2 {
            let ch_phase = if ch == 0 {
                phase
            } else {
                (phase + 0.25 * stereo_width).rem_euclid(1.0)
            };
            let (sin_v, cos_v) = (TAU * ch_phase).sin_cos();
            let d1_ms = (delay_ms + mod_depth_ms * sin_v).max(0.5);
            let d2_ms = (delay_ms + mod_depth_ms * cos_v).max(0.5);
            let d1 = (d1_ms * sr / 1_000.0).clamp(1.0, max_delay_frames);
            let d2 = (d2_ms * sr / 1_000.0).clamp(1.0, max_delay_frames);
            let v1 = read_ring_cubic(&self.scratch, s.next, s.valid, d1, ch);
            let v2 = read_ring_cubic(&self.scratch, s.next, s.valid, d2, ch);
            let raw = 0.5 * (v1 + v2);

            // 1-pole high-pass on wet path (state stores low-pass component).
            let lp_hpf = hex_dsp::flush_quiet(raw + hpf_pole * (s.hpf_state[ch] - raw));
            s.hpf_state[ch] = lp_hpf;
            let hp = if hpf_hz <= 20.001 { raw } else { raw - lp_hpf };

            // 1-pole analog warmth low-pass.
            let toned = if lpf_pole == 0.0 {
                s.warmth_state[ch] = 0.0;
                hp
            } else {
                let w = hex_dsp::flush_quiet(hp + lpf_pole * (s.warmth_state[ch] - hp));
                s.warmth_state[ch] = w;
                w
            };

            wet_out[ch] = toned;
            // Softly bound feedback sum to guarantee stability even at extreme inputs.
            let fb_in = hex_dsp::flush_quiet(inputs[ch] + feedback * toned);
            to_write[ch] = fb_in as f32;
        }

        self.scratch[s.next] = to_write;
        s.next = (s.next + 1) % len;
        if s.valid < len {
            s.valid += 1;
        }
        self.dirty_len = (self.dirty_len + 1).min(len);

        if silent && s.silent_frames >= self.tail_limit {
            s.valid = 0;
            s.hpf_state = [0.0; 2];
            s.warmth_state = [0.0; 2];
            s.stale_slots = true;
        }

        if bypassed {
            return;
        }
        if dry_wet == 0.0 {
            return;
        }
        if dry_wet == 1.0 {
            frame[0] = wet_out[0] as f32;
            frame[1] = wet_out[1] as f32;
        } else {
            let dry = 1.0 - dry_wet;
            frame[0] = (in_l * dry + wet_out[0] * dry_wet) as f32;
            frame[1] = (in_r * dry + wet_out[1] * dry_wet) as f32;
        }
    }

    fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        if !self.open {
            self.begin_block();
        }
        let ramp_frames = self.ramp_frames();
        let mut resolved = self.speculative.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.speculative
                    .controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let effective = next.effective();
                self.speculative.retarget(effective, ramp_frames);
                self.speculative.controls.install(next);
            }
            self.process_frame(frame, bypassed);
        }
        Ok(())
    }

    fn inherit_scalars(&mut self, old: ChorusScalar, old_settings: ChorusSettings) {
        self.pristine = false;
        let ramp_frames = self.ramp_frames();
        let mut inherited = old;
        let mut controls = self.primary.controls;
        controls.inherit(&old.controls);
        inherited.controls = controls;
        if self.settings != old_settings {
            inherited.retarget(self.settings, ramp_frames);
        }
        self.primary = inherited;
        self.speculative = inherited;
        self.dirty_start = inherited.next;
        self.dirty_len = 0;
        self.open = false;
    }
}

impl OwnedProcessorRuntime for ChorusRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.primary.controls.supports(&BINDINGS, id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(with_clock process_automation);

    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        let (inline, [a, b]) = self.control_storage()?;
        inline.checked_add(a)?.checked_add(b)
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.dirty_start = self.speculative.next;
        self.dirty_len = 0;
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        if !self.open {
            self.begin_block();
        }
        for frame in frames {
            self.process_frame(frame, false);
        }
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block(frames, resources);
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        if !self.open {
            self.begin_block();
        }
        for frame in frames {
            self.process_frame(frame, true);
        }
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        self.primary = self.speculative;
        self.dirty_len = 0;
        self.open = false;
        self.pristine = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        self.speculative = self.primary;
        self.dirty_len = 0;
        self.open = false;
    }
    fn reset(&mut self) {
        self.pristine = false;
        let admitted = std::array::from_fn(|i| self.primary.controls.admitted(i));
        let mut s = ChorusScalar::new(self.settings, self.rate, admitted, self.tail_limit);
        s.stale_slots = true;
        self.primary = s;
        self.speculative = s;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.is_quiescent(self.tail_limit)
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.speculative.is_quiescent(self.tail_limit)
    }
    fn can_inherit_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| !self.open && !old.open && self.rate == old.rate)
    }
    fn inherit_from(&mut self, old: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            let len = old.committed.len();
            let start = (old.primary.next + len - old.primary.valid) % len;
            Self::copy_interval(
                &old.committed,
                &mut self.committed,
                start,
                old.primary.valid,
            );
            Self::copy_interval(&old.committed, &mut self.scratch, start, old.primary.valid);
            self.inherit_scalars(old.primary, old.settings);
            self.primary.stale_slots = true;
            self.speculative.stale_slots = true;
        }
    }
    fn transfer_history_from(&mut self, old: &mut dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any_mut().and_then(|v| v.downcast_mut::<Self>()) {
            std::mem::swap(&mut self.committed, &mut old.committed);
            std::mem::swap(&mut self.scratch, &mut old.scratch);
            self.inherit_scalars(old.primary, old.settings);
            old.reset();
        }
    }
    fn can_fork_timing_without_signal_history_from(
        &self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.pristine
                && self.primary.valid == 0
                && self.speculative.valid == 0
                && !self.open
                && !old.open
                && self.rate == old.rate
                && self.settings == old.settings
                && self.committed.len() == old.committed.len()
        })
    }
    fn fork_timing_without_signal_history_from(
        &mut self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        if !self.can_fork_timing_without_signal_history_from(previous) {
            return false;
        }
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        let mut controls = self.primary.controls;
        controls.inherit(&old.primary.controls);
        let mut scalar = old.primary;
        scalar.controls = controls;
        scalar.valid = 0;
        scalar.hpf_state = [0.0; 2];
        scalar.warmth_state = [0.0; 2];
        scalar.silent_frames = self.tail_limit;
        self.primary = scalar;
        self.speculative = scalar;
        self.dirty_start = scalar.next;
        self.dirty_len = 0;
        self.pristine = false;
        true
    }
    fn can_split_tail_input_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_fork_timing_without_signal_history_from(previous)
    }
    fn can_retarget_parameters_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(old)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;
