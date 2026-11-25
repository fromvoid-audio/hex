//! Fixed-inline filter history, stereo LFO phase, linked envelope follower,
//! bounded parameter automation/modulation, and deterministic finite-tail drain.
//! The host copies committed state into scratch before each transaction.

use super::*;
use crate::auto_pan::Waveform;
use hex_dsp::{EnvelopeFollower, OnePoleSmoother, SvfFilter, SvfFrame, SvfMode};
use hex_lattice::{
    DeviceResources, NativeParameterAutomation, NativeProcessorRuntime, OutputTail, StereoFrame,
};
use std::{any::Any, f64::consts::PI};

/// Worst-case resonant SVF tail ceiling in seconds (even at 20 Hz, 100% resonance, 24 dB).
const MAX_TAIL_SECONDS: u32 = 6;

fn max_tail_frames_u32(rate: u32) -> u32 {
    rate.saturating_mul(MAX_TAIL_SECONDS)
}

fn build_frame(settings: Settings, rate: u32) -> Result<SvfFrame, DeviceContractError> {
    let sample_rate = f64::from(rate);
    let frequency = settings.float(CUTOFF).min(0.45 * sample_rate);
    let mode = SvfMode::from_index(settings.0[TYPE])
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    Ok(SvfFrame::clean_filter(
        (PI * frequency / sample_rate).tan(),
        mode,
        settings.float(RESONANCE) / 100.0,
        settings.float(MORPH) / 100.0,
        settings.float(MIX) / 100.0,
        f64::from(settings.0[SLOPE]),
    ))
}

#[derive(Clone, Copy)]
pub struct PreparedFilter {
    settings: Settings,
    admitted: [bool; COUNT],
    pub(super) frame: SvfFrame,
    rate: u32,
    oscillator: Waveform,
    phase_step: f64,
    phase_offset: f64,
    lfo_depth: f64,
    envelope_depth: f64,
    envelope: EnvelopeFollower,
    modulation_smoother: OnePoleSmoother,
}

impl PreparedFilter {
    pub(super) fn new(
        settings: Settings,
        rate: u32,
        admitted: [bool; COUNT],
    ) -> Result<Self, DeviceContractError> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let sample_rate = f64::from(rate);
        let frame = build_frame(settings, rate)?;
        let modulation_smoother = OnePoleSmoother::new(0.0, rate, 0.002)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        Ok(Self {
            settings,
            admitted,
            frame,
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
            modulation_smoother,
        })
    }

    fn supports(&self, id: ParameterId) -> bool {
        parameter_index(id).is_ok_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedFilter {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.supports(id)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|binding| binding.descriptor)
    }
    fn parameter_modulation_semantics(&self, id: ParameterId) -> Option<ParameterSemantics> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|binding| binding.semantics)
    }
    fn runtime_version(&self) -> u32 {
        VERSION
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<FilterRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(FilterRuntime::new(*self))
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(max_tail_frames_u32(self.rate)),
        }
    }
}

/// Copy-bounded payload charged by the sealed host storage ledger.
#[derive(Clone, Copy)]
pub struct FilterRuntime {
    base: PreparedFilter,
    controls: crate::parameter_binding::Controls<Settings, COUNT>,
    target_frame: SvfFrame,
    frame: SvfFrame,
    ramp: u32,
    phase_step: f64,
    phase_offset: f64,
    lfo_depth: f64,
    envelope_depth: f64,
    lane: [SvfFilter; 2],
    envelope: EnvelopeFollower,
    phase: f64,
    modulation: [OnePoleSmoother; 2],
    silent_frames: u32,
}

impl FilterRuntime {
    fn new(base: PreparedFilter) -> Self {
        Self {
            base,
            controls: crate::parameter_binding::Controls::new(base.settings, base.admitted),
            target_frame: base.frame,
            frame: base.frame,
            ramp: 0,
            phase_step: base.phase_step,
            phase_offset: base.phase_offset,
            lfo_depth: base.lfo_depth,
            envelope_depth: base.envelope_depth,
            lane: [SvfFilter::new(); 2],
            envelope: base.envelope,
            phase: 0.0,
            modulation: [base.modulation_smoother; 2],
            silent_frames: max_tail_frames_u32(base.rate),
        }
    }

    fn clear_history(&mut self) {
        self.lane = [SvfFilter::new(); 2];
        self.envelope.reset();
        self.modulation = [self.base.modulation_smoother; 2];
    }

    fn retarget_from_settings(
        &mut self,
        settings: Settings,
        previous: Settings,
    ) -> Result<(), DeviceContractError> {
        let rate = self.base.rate;
        let sample_rate = f64::from(rate);
        if settings.0[CUTOFF] != previous.0[CUTOFF]
            || settings.0[RESONANCE] != previous.0[RESONANCE]
            || settings.0[MORPH] != previous.0[MORPH]
            || settings.0[MIX] != previous.0[MIX]
        {
            let next_frame = build_frame(settings, rate)?;
            if next_frame != self.target_frame {
                self.target_frame = next_frame;
                self.ramp = rate.div_ceil(200);
            }
        }
        self.phase_step = settings.float(LFO_RATE) / sample_rate;
        self.phase_offset = settings.float(LFO_PHASE) / 360.0;
        self.lfo_depth = settings.float(LFO_AMOUNT);
        self.envelope_depth = settings.float(ENV_AMOUNT);
        if settings.0[ENV_ATTACK] != previous.0[ENV_ATTACK]
            || settings.0[ENV_RELEASE] != previous.0[ENV_RELEASE]
        {
            let mut updated = EnvelopeFollower::new(
                rate,
                settings.float(ENV_ATTACK) * 0.001,
                settings.float(ENV_RELEASE) * 0.001,
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
            updated.inherit_envelope(&self.envelope);
            self.envelope = updated;
        }
        Ok(())
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        if self.ramp != 0 {
            self.frame = self
                .frame
                .interpolate(self.target_frame, 1.0 / f64::from(self.ramp));
            self.ramp -= 1;
        }
        let input = frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
        let limit = max_tail_frames_u32(self.base.rate);
        let silent_input = input == [0.0; 2];
        if silent_input {
            if self.silent_frames >= limit {
                self.phase = (self.phase + self.phase_step).fract();
                *frame = [0.0; 2];
                return;
            }
            self.silent_frames = self.silent_frames.saturating_add(1).min(limit);
            if self.silent_frames == limit {
                self.clear_history();
                self.phase = (self.phase + self.phase_step).fract();
                *frame = [0.0; 2];
                return;
            }
        } else {
            self.silent_frames = 0;
        }

        let mut envelope = [0.0];
        // Identical shapes and timing for whole or partitioned blocks.
        let _ = self
            .envelope
            .render(std::slice::from_ref(&input), &mut envelope);
        let envelope = f64::from(envelope[0]) * self.envelope_depth;
        let fade_frames = self.base.rate.div_ceil(200).max(1);
        let remaining = limit.saturating_sub(self.silent_frames);
        let tail_gain = if silent_input && remaining < fade_frames {
            f64::from(remaining) / f64::from(fade_frames)
        } else {
            1.0
        };
        for (channel, sample) in frame.iter_mut().enumerate() {
            let lfo = if self.lfo_depth == 0.0 {
                0.0
            } else {
                let phase = (self.phase + channel as f64 * self.phase_offset).fract();
                (2.0 * self.base.oscillator.lfo(phase) - 1.0) * self.lfo_depth
            };
            let offset = self.modulation[channel].advance_toward(envelope + lfo);
            let coefficients = if offset.abs() > 1e-6 {
                self.frame.with_cutoff_offset(offset, self.base.rate)
            } else {
                self.modulation[channel].reset(0.0);
                self.frame
            };
            let output =
                self.lane[channel].process(f64::from(input[channel]), coefficients) * tail_gain;
            *sample = output.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32;
        }
        if silent_input
            && self.lane.into_iter().all(SvfFilter::is_quiet)
            && frame[0] == 0.0
            && frame[1] == 0.0
        {
            self.clear_history();
            self.silent_frames = limit;
        }
        self.phase = (self.phase + self.phase_step).fract();
    }

    fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let old = self.controls.effective();
                self.retarget_from_settings(next.effective(), old)?;
                self.controls.install(next);
            }
            let original = *frame;
            self.process_frame(frame);
            if bypassed {
                *frame = original;
            }
        }
        Ok(())
    }
}

impl NativeProcessorRuntime for FilterRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.controls.supports(&BINDINGS, id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automated);

    fn process_block(&mut self, frames: &mut [StereoFrame], _resources: &dyn DeviceResources) {
        for frame in frames {
            self.process_frame(frame);
        }
    }

    fn reset(&mut self) {
        *self = Self::new(self.base);
    }
    fn is_quiescent(&self) -> bool {
        self.lane.into_iter().all(SvfFilter::is_quiet)
            && self.silent_frames >= max_tail_frames_u32(self.base.rate)
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
            self.lane = old.lane;
            self.envelope.inherit_envelope(&old.envelope);
            self.phase = old.phase;
            self.silent_frames = old.silent_frames;
            for (dst, src) in self.modulation.iter_mut().zip(&old.modulation) {
                dst.inherit_value(src);
            }
            if BINDINGS
                .iter()
                .any(|b| self.controls.supports(&BINDINGS, b.descriptor.id))
                && BINDINGS
                    .iter()
                    .any(|b| old.controls.supports(&BINDINGS, b.descriptor.id))
            {
                let previous = self.controls.effective();
                self.controls.inherit(&old.controls);
                let inherited = self.controls.effective();
                let _ = self.retarget_from_settings(inherited, previous);
            }
            self.frame = old.frame;
            self.ramp = if self.target_frame == old.frame {
                old.ramp
            } else {
                self.base.rate.div_ceil(200)
            };
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
