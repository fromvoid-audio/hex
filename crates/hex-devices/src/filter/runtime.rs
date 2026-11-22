//! Fixed-inline filter history, stereo LFO phase and linked envelope follower.
//! The host copies committed state into scratch before each transaction.

use super::*;
use crate::{
    auto_pan::Waveform,
    operator_filter::{Carrier, Frame},
};
use hex_dsp::EnvelopeFollower;
use hex_lattice::{DeviceResources, NativeProcessorRuntime, OutputTail, StereoFrame};
use std::{any::Any, f64::consts::PI};

#[derive(Clone, Copy)]
pub struct PreparedFilter {
    pub(super) frame: Frame,
    rate: u32,
    oscillator: Waveform,
    phase_step: f64,
    phase_offset: f64,
    lfo_depth: f64,
    envelope_depth: f64,
    envelope: EnvelopeFollower,
    modulation_step: f64,
}

impl PreparedFilter {
    pub(super) fn new(settings: Settings, rate: u32) -> Result<Self, DeviceContractError> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let sample_rate = f64::from(rate);
        let frequency = settings.float(CUTOFF).min(0.45 * sample_rate);
        Ok(Self {
            frame: Frame::clean_filter(
                (PI * frequency / sample_rate).tan(),
                settings.0[TYPE],
                settings.float(RESONANCE) / 100.0,
                settings.float(MORPH) / 100.0,
                settings.float(MIX) / 100.0,
                f64::from(settings.0[SLOPE]),
            ),
            rate,
            oscillator: Waveform::from_choice(settings.0[LFO_SHAPE])?,
            phase_step: settings.float(LFO_RATE) / sample_rate,
            phase_offset: settings.float(LFO_PHASE) / 360.0,
            lfo_depth: settings.float(LFO_AMOUNT),
            envelope_depth: settings.float(ENV_AMOUNT),
            envelope: EnvelopeFollower::new(
                rate,
                settings.float(ENV_ATTACK) * 0.001,
                settings.float(ENV_RELEASE) * 0.001,
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?,
            modulation_step: -(-1.0 / (sample_rate * 0.002)).exp_m1(),
        })
    }
}

impl PreparedNativeProcessor for PreparedFilter {
    fn runtime_version(&self) -> u32 {
        VERSION
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<FilterRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        let runtime = FilterRuntime::new(*self);
        // The sealed storage ledger charges this Copy-bounded payload.
        fn inline<T: Copy>(_value: &T) {}
        inline(&runtime);
        Box::new(runtime)
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

#[derive(Clone, Copy)]
pub struct FilterRuntime {
    base: PreparedFilter,
    frame: Frame,
    ramp: u32,
    lane: [Carrier; 2],
    envelope: EnvelopeFollower,
    phase: f64,
    modulation: [f64; 2],
}

impl FilterRuntime {
    fn new(base: PreparedFilter) -> Self {
        Self {
            base,
            frame: base.frame,
            ramp: 0,
            lane: [Carrier::default(); 2],
            envelope: base.envelope,
            phase: 0.0,
            modulation: [0.0; 2],
        }
    }
}

impl NativeProcessorRuntime for FilterRuntime {
    fn process_block(&mut self, frames: &mut [StereoFrame], _resources: &dyn DeviceResources) {
        for frame in frames {
            if self.ramp != 0 {
                self.frame = self
                    .frame
                    .interpolate(self.base.frame, 1.0 / f64::from(self.ramp));
                self.ramp -= 1;
            }
            let input = frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
            let mut envelope = [0.0];
            // Identical shapes and timing for whole or partitioned blocks.
            let _ = self
                .envelope
                .render(std::slice::from_ref(&input), &mut envelope);
            let envelope = f64::from(envelope[0]) * self.base.envelope_depth;
            for channel in 0..2 {
                let lfo = if self.base.lfo_depth == 0.0 {
                    0.0
                } else {
                    let phase = (self.phase + channel as f64 * self.base.phase_offset).fract();
                    (2.0 * self.base.oscillator.lfo(phase) - 1.0) * self.base.lfo_depth
                };
                let offset = &mut self.modulation[channel];
                *offset += (envelope + lfo - *offset) * self.base.modulation_step;
                let coefficients = if offset.abs() > 1e-6 {
                    self.frame.with_cutoff_offset(*offset, self.base.rate)
                } else {
                    *offset = 0.0;
                    self.frame
                };
                let output = self.lane[channel].process(f64::from(input[channel]), coefficients);
                frame[channel] = output.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32;
            }
            self.phase = (self.phase + self.base.phase_step).fract();
        }
    }

    fn reset(&mut self) {
        *self = Self::new(self.base);
    }
    fn is_quiescent(&self) -> bool {
        self.lane.into_iter().all(Carrier::is_quiet)
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.base.rate == self.base.rate)
    }
    fn can_retarget_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(old)
    }
    fn can_retarget_source_parameters_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(old)
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old
            .as_any()
            .downcast_ref::<Self>()
            .filter(|old| old.base.rate == self.base.rate)
        {
            self.frame = old.frame;
            self.ramp = if self.base.frame == old.base.frame {
                old.ramp
            } else {
                self.base.rate.div_ceil(200)
            };
            self.lane = old.lane;
            self.envelope.inherit_envelope(&old.envelope);
            self.phase = old.phase;
            self.modulation = old.modulation;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
