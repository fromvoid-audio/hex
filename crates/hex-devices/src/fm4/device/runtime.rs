//! Fixed FM4 voices and carrier controls; native host admission is separate.
//!
//! Control prepares all 128 tuned step entries (invalid/Nyquist keys remain
//! explicitly unavailable). One twin owns 64 inline occurrence slots; batches
//! admit at most 259 events. Released slots remain occupied until the exact
//! authored ADSR tail ends. Capacity refusal is read-only; there is no stealing.
//! Events at equal offsets retain order; controller expressions have no FM4 law.
//!
//! Carrier weights apply before note events. Cold/reset controls adopt directly;
//! later changes use a five-millisecond linear ramp, retargeted from the current
//! effective weight. This is a Hex policy, not measured reference behavior.
//! Original levels remain available to reveal previously inaudible operators.
//! Voice age/gate and the original tuned phase-step law are unchanged by carrier
//! controls. Per-voice f32 source samples receive occurrence gain before f64 sum.
//! Complete controls, effective pruning and voices participate in twin history.
//! A prepared retarget may change same-rate tuning and Base carriers only.
//! Each retained voice carries four modulo-2^64 phase offsets; retarget adds
//! (source_step-target_step)*age without changing age/gate/velocity. Both twins
//! validate exact target proof, complete noncarrier authored law and every live
//! target pitch before mutation. Effective carrier ramps survive the handoff;
//! the next segment selects the target's Base or supplied authored values.
//! No assets, queues, host publication, event scheduler or new resource owner.
//! Native definition admission and parameter metadata remain unchanged.
use super::*;
use crate::fm4::{
    PreparedEnvelope, PreparedFmConfiguration, audible_operator_ancestors, prepared_steps_frequency,
};
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MutableInstrumentRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError as Error,
    NativeParameterAutomation, PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
    StereoFrame,
};
use std::any::Any;
const VOICES: usize = 64;
const EVENTS: usize = 259;
const CARRIER_START: usize = (OSCILLATOR_FIELDS + ROUTING_FIELDS) * size_of::<f32>();
const CARRIER_END: usize = CARRIER_START + CARRIER_FIELDS * size_of::<f32>();
const CARRIERS: [ParameterDescriptor; 4] = [
    PARAMETERS[24],
    PARAMETERS[25],
    PARAMETERS[26],
    PARAMETERS[27],
];

/// Control-only component preparation through FM4's existing validated state
/// and tuning contract. This does not advertise native host automation support.
impl Fm4Definition {
    pub fn prepare_carrier_runtime(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedMutableInstrumentRuntime>, DeviceContractError> {
        let settings = Fm4Settings::from_state(state)?;
        let (oscillators, routing, envelope) = settings.kernel_parts()?;
        let sample_rate = context.sample_rate;
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut steps = [None; 128];
        for (pitch, slot) in steps.iter_mut().enumerate() {
            let frequency = context.tuning.frequencies()[pitch];
            *slot =
                prepared_steps_frequency(sample_rate, frequency, &oscillators.map(|osc| osc.ratio))
                    .ok();
        }
        let base = PreparedFmConfiguration::new(oscillators, routing);
        // Keep original levels for operators that a later carrier can reveal.
        let mut full = PreparedFmConfiguration::new(
            oscillators,
            super::super::Routing {
                carriers: [1.0; 4],
                ..routing
            },
        );
        full.routing.carriers = routing.carriers;
        let frames = |seconds: f64| (seconds * f64::from(sample_rate)).round() as u64;
        let mut identity = [0; PAYLOAD_BYTES];
        identity.copy_from_slice(state.payload());
        Ok(Box::new(Factory {
            config: Config {
                identity,
                tuning: *context.tuning,
                sample_rate,
                steps,
                base,
                full,
                envelope: PreparedEnvelope {
                    attack: frames(envelope.attack_seconds),
                    decay: frames(envelope.decay_seconds),
                    sustain: envelope.sustain,
                    release: frames(envelope.release_seconds),
                },
                table: crate::shared_sine_table(),
                smoothing: ((u64::from(sample_rate) * 5 + 500) / 1000) as u32,
            },
        }))
    }
}
#[derive(Clone, Copy)]
struct Config {
    identity: [u8; PAYLOAD_BYTES],
    tuning: hex_lattice::TuningTable,
    sample_rate: u32,
    steps: [Option<[u64; 4]>; 128],
    base: PreparedFmConfiguration,
    full: PreparedFmConfiguration,
    envelope: PreparedEnvelope,
    table: &'static crate::SineTable,
    smoothing: u32,
}
impl Config {
    fn same_target(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.tuning == other.tuning
            && self.sample_rate == other.sample_rate
    }
}
/// Exact immutable target authority; custody remains with the prepared graph.
struct Retarget {
    identity: [u8; PAYLOAD_BYTES],
    tuning: hex_lattice::TuningTable,
    sample_rate: u32,
}
impl PreparedInstrumentHistoryRetarget for Retarget {
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Factory {
    config: Config,
}
#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    pitch: u8,
    velocity: u8,
    age: u64,
    gate: Option<u64>,
    phase_offsets: [u64; 4],
}
impl Voice {
    fn advance(&mut self, frames: u64, release: u64) {
        if self.id == 0 {
            return;
        }
        self.age = self.age.saturating_add(frames);
        if self
            .gate
            .is_some_and(|gate| self.age.saturating_sub(gate) >= release)
        {
            *self = Self::default();
        }
    }
}
#[derive(Clone, Copy)]
struct Control {
    current: f64,
    target: f64,
    remaining: u32,
}
impl Control {
    fn sample(&mut self, target: f64, frames: u32, cold: bool) -> f64 {
        if cold {
            self.current = target;
            self.target = target;
            self.remaining = 0;
        } else if target != self.target {
            self.target = target;
            self.remaining = frames;
        }
        if self.remaining != 0 {
            self.current += (self.target - self.current) / f64::from(self.remaining);
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        self.current
    }
}
struct Runtime {
    config: Config,
    voices: [Voice; VOICES],
    controls: [Control; 4],
    effective: PreparedFmConfiguration,
    cold: bool,
}
impl Runtime {
    fn new(config: Config) -> Self {
        Self {
            config,
            voices: [Voice::default(); VOICES],
            controls: config.base.routing.carriers.map(|value| Control {
                current: value,
                target: value,
                remaining: 0,
            }),
            effective: config.base,
            cold: true,
        }
    }
    fn apply(
        config: &Config,
        voices: &mut [Voice; VOICES],
        event: NativeInstrumentEvent,
    ) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if config
                    .steps
                    .get(usize::from(pitch))
                    .is_none_or(Option::is_none)
                    || !(1..=127).contains(&velocity)
                    || occurrence_id == 0
                    || voices.iter().any(|voice| voice.id == occurrence_id)
                {
                    return Err(Error::InvalidEventBatch);
                }
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = Voice {
                    id: occurrence_id,
                    pitch,
                    velocity,
                    age: 0,
                    gate: None,
                    phase_offsets: [0; 4],
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = voices.iter_mut().find(|voice| voice.id == occurrence_id) {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                    voice.advance(0, config.envelope.release);
                }
            }
            NativeInstrumentEvent::Expression { .. } => {}
        }
        Ok(())
    }
    fn set_carriers(&mut self, carriers: [f64; 4]) {
        self.effective = self.config.full;
        self.effective.routing.carriers = carriers;
        let audible = audible_operator_ancestors(&self.effective.levels, &self.effective.routing);
        for (index, audible) in audible.into_iter().enumerate() {
            if !audible {
                self.effective.levels[index] = 0.0;
            }
        }
    }
    fn render(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
    ) {
        let mut next_event = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            let carriers = std::array::from_fn(|index| {
                let target =
                    match values.and_then(|values| values.value_at(CARRIERS[index].id, offset)) {
                        Some(ParameterValue::Float(value)) => f64::from(value),
                        _ => self.config.base.routing.carriers[index],
                    };
                self.controls[index].sample(target, self.config.smoothing, self.cold)
            });
            if carriers != self.effective.routing.carriers {
                self.set_carriers(carriers);
            }
            self.cold = false;
            while let Some(event) = events
                .get(next_event)
                .filter(|event| event.frame_offset() == offset)
            {
                // Complete preflight proved this infallible for this exact batch.
                let _ = Self::apply(&self.config, &mut self.voices, *event);
                next_event += 1;
            }
            let mut sum = 0.0_f64;
            for voice in &mut self.voices {
                if voice.id == 0 {
                    continue;
                }
                let Some(steps) = self.config.steps[usize::from(voice.pitch)] else {
                    continue;
                };
                let amplitude = self
                    .config
                    .envelope
                    .level(voice.age, voice.gate.unwrap_or(u64::MAX))
                    * (f64::from(voice.velocity) / 127.0);
                let mut configuration = self.effective;
                for (phase, offset) in configuration.phases.iter_mut().zip(voice.phase_offsets) {
                    *phase = phase.wrapping_add(offset);
                }
                let sample = if amplitude == 0.0 {
                    0.0
                } else {
                    (configuration.routed_output::<false>(
                        self.config.table,
                        voice.age,
                        &steps,
                        [1.0; 4],
                    ) * amplitude) as f32
                };
                let gain = gains
                    .binary_search_by_key(&voice.id, |gain| gain.occurrence_id())
                    .ok()
                    .map_or(1.0, |index| gains[index].value_at_offset(offset as u64));
                sum += f64::from(sample) * gain;
                voice.advance(1, self.config.envelope.release);
            }
            frame[0] += sum as f32;
            frame[1] += sum as f32;
        }
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &CARRIERS
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(size_of::<Self>())
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        2
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Retarget {
                identity: self.config.identity,
                tuning: self.config.tuning,
                sample_rate: self.config.sample_rate,
            },
        )))
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        EVENTS
    }
    fn max_active_occurrences(&self) -> usize {
        VOICES
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.config.envelope.release
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime::new(self.config))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        Some(self)
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn active_occurrence_ids(&self, output: &mut [u64]) -> Result<usize, Error> {
        let count = self.voices.iter().filter(|voice| voice.id != 0).count();
        if count > output.len() {
            return Err(Error::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output
            .iter_mut()
            .zip(self.voices.iter().filter(|voice| voice.id != 0))
        {
            *slot = voice.id;
        }
        output[..count].sort_unstable();
        Ok(count)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > EVENTS {
            return Err(Error::EventCapacityExceeded);
        }
        let mut voices = self.voices;
        let mut previous = 0;
        for event in events {
            let offset = event.frame_offset();
            if offset < previous || offset >= frame_count {
                return Err(Error::InvalidEventBatch);
            }
            for voice in &mut voices {
                voice.advance((offset - previous) as u64, self.config.envelope.release);
            }
            Self::apply(&self.config, &mut voices, *event)?;
            previous = offset;
        }
        Ok(())
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), Error> {
        if gains.iter().any(|gain| !self.voices.iter().any(|voice| voice.id == gain.occurrence_id()) && !events.iter().any(|event| matches!(event, NativeInstrumentEvent::NoteOn { occurrence_id, .. } if *occurrence_id == gain.occurrence_id()))) { return Err(Error::InvalidOccurrenceGain); }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, &[], None);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, gains, None);
    }
    fn reset(&mut self) {
        *self = Self::new(self.config);
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| voice.id == 0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.config.same_target(&previous.config))
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
            self.controls = previous.controls;
            self.effective = previous.effective;
            self.cold = previous.cold;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<Retarget>(),
        ) else {
            return false;
        };
        (self.config.identity == proof.identity
            && self.config.tuning == proof.tuning
            && self.config.sample_rate == proof.sample_rate)
            && self.config.sample_rate == previous.config.sample_rate
            && self.config.identity[..CARRIER_START] == previous.config.identity[..CARRIER_START]
            && self.config.identity[CARRIER_END..] == previous.config.identity[CARRIER_END..]
            && previous
                .voices
                .iter()
                .filter(|voice| voice.id != 0)
                .all(|voice| {
                    self.config
                        .steps
                        .get(usize::from(voice.pitch))
                        .is_some_and(Option::is_some)
                })
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        _: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
            for voice in self.voices.iter_mut().filter(|voice| voice.id != 0) {
                let (Some(source), Some(target)) = (
                    previous.config.steps[usize::from(voice.pitch)],
                    self.config.steps[usize::from(voice.pitch)],
                ) else {
                    continue;
                };
                for ((offset, source), target) in
                    voice.phase_offsets.iter_mut().zip(source).zip(target)
                {
                    // Oscillator phase is modulo 2^64, including repeated retunes.
                    *offset =
                        offset.wrapping_add(source.wrapping_sub(target).wrapping_mul(voice.age));
                }
            }
            self.controls = previous.controls;
            self.set_carriers(previous.controls.map(|control| control.current));
            self.cold = previous.cold;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl InstrumentParameterRuntime for Runtime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        &CARRIERS
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        _: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, gains, Some(values));
    }
}
#[cfg(test)]
mod tests;
