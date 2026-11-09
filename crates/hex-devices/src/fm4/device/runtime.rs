//! Fixed FM4 voices, oscillator phases, levels and carrier controls used by the native host.
//!
//! Control prepares all 128 tuned step entries (invalid/Nyquist keys remain
//! explicitly unavailable). One twin owns 64 inline occurrence slots; batches
//! admit at most 259 events. Released slots remain occupied until the exact
//! authored ADSR tail ends. Capacity refusal is read-only; there is no stealing.
//! Events at equal offsets retain order. A prepared pressure route affects the
//! matching held voice only, retains its last pressure through release, and
//! clears on retirement or reset. Channel-wide expressions have no FM4 law.
//!
//! Carrier weights apply before note events. Cold/reset controls adopt directly;
//! later authored changes use a five-millisecond linear ramp, retargeted from
//! the current effective weight. A routed carrier follows its per-sample
//! canonical value directly. Removing that route resumes the authored ramp
//! from the last effective weight.
//! Each sample's oscillator levels are selected before pruning or rendering.
//! Unpruned effective levels remain available to a newly audible carrier and
//! to a detached release.
//! Sample-rate phases are absolute offsets in turns before the tuned
//! phase step and any per-voice retune correction. An unassigned phase
//! resumes the authored offset at the first sample of that segment.
//! Voice age/gate and the original tuned phase-step law are unchanged by carrier
//! controls. Per-voice f32 source samples receive occurrence gain before f64 sum.
//! Complete controls, effective pruning and voices participate in twin history.
//! Rendering collects active slot indices once per segment and after event
//! frames, preserving ascending slot summation order. Retirement can leave an
//! inert index until the next collection; the zero-ID check prevents rendering
//! it. Ordinary segments with exact settled Base controls skip carrier updates.
//! These are derived stack views, not additional runtime history or custody.
//! A prepared retarget adopts the complete same-rate target Base law. Ratio or
//! tuning changes preserve oscillator phase; authored phase changes apply the
//! new phase explicitly. Level/routing and attack/decay/sustain take effect at
//! the retained note age, so coefficient edits may be audible discontinuities.
//! Each retained voice carries four modulo-2^64 phase offsets; retarget adds
//! (source_step-target_step)*age without changing age/gate/velocity. Both twins
//! validate exact target proof, same sample rate and every live target pitch
//! before mutation. Each note starts with its authored release ceiling; retarget
//! takes min(previous ceiling,target release) to respect the host's existing
//! occurrence deadline and target factory bound. A shorter release can retire
//! an already-released note immediately. Release increases affect new notes;
//! extending a carried note requires a separate host-deadline contract. Effective carrier ramps survive the handoff;
//! the next segment selects the target's Base or supplied authored values.
//! No assets, queues, host publication, event scheduler or new resource owner.
//! Parameter automation metadata is exposed separately from this runtime.
use super::*;
use crate::fm4::{
    PreparedEnvelope, PreparedFmConfiguration, audible_operator_ancestors, phase_from_turns,
    prepared_steps_frequency,
};
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MutableInstrumentRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError as Error,
    NativeParameterAutomation, PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
    PreparedPressureRoute, StereoFrame,
};
use std::any::Any;
const VOICES: usize = 64;
const EVENTS: usize = 259;
const CARRIERS: [ParameterDescriptor; 4] = [
    PARAMETERS[24],
    PARAMETERS[25],
    PARAMETERS[26],
    PARAMETERS[27],
];

const LEVELS: [ParameterDescriptor; 4] =
    [PARAMETERS[1], PARAMETERS[4], PARAMETERS[7], PARAMETERS[10]];
const PHASES: [ParameterDescriptor; 4] =
    [PARAMETERS[2], PARAMETERS[5], PARAMETERS[8], PARAMETERS[11]];
const TARGETS: [ParameterDescriptor; 12] = [
    LEVELS[0],
    PHASES[0],
    LEVELS[1],
    PHASES[1],
    LEVELS[2],
    PHASES[2],
    LEVELS[3],
    PHASES[3],
    CARRIERS[0],
    CARRIERS[1],
    CARRIERS[2],
    CARRIERS[3],
];

/// Control-only component preparation through FM4's existing validated state
/// and tuning contract. This does not advertise native host automation support.
impl Fm4Definition {
    /// Prepare the same fixed factory used by registered native FM4 playback.
    pub fn prepare_carrier_runtime(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedMutableInstrumentRuntime>, DeviceContractError> {
        Ok(Box::new(Factory::prepare(state, context)?))
    }

    /// Control-side grant of one inline per-occurrence pressure assignment.
    /// The same native twins retain voices, authored controls and phase.
    pub fn prepare_pressure_runtime(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        route: PreparedPressureRoute,
    ) -> Result<Box<dyn PreparedMutableInstrumentRuntime>, DeviceContractError> {
        Ok(Box::new(
            Factory::prepare(state, context)?.with_note_pressure(None, Some(route))?,
        ))
    }
}
impl Factory {
    pub(super) fn with_note_pressure(
        mut self,
        source: Option<hex_lattice_contract::ModulationSourceId>,
        route: Option<PreparedPressureRoute>,
    ) -> Result<Self, DeviceContractError> {
        self.config.pressure_source = source;
        self.config.pressure = route
            .map(|route| {
                let index = CARRIERS
                    .iter()
                    .position(|descriptor| *descriptor == route.descriptor())
                    .filter(|_| {
                        modulation_semantics(route.descriptor().id) == Some(route.semantics())
                    })
                    .ok_or(DeviceContractError::UnsupportedProcessingClass)?;
                let mut routing = self.config.full.routing;
                routing.carriers = [0.0; 4];
                routing.carriers[index] = 1.0;
                let required = audible_operator_ancestors(&[1.0; 4], &routing);
                Ok(PressureRoute {
                    index,
                    route,
                    required,
                })
            })
            .transpose()?;
        Ok(self)
    }
    pub(super) fn validate_note_on(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<(), DeviceContractError> {
        if (1..=127).contains(&velocity)
            && self
                .config
                .steps
                .get(usize::from(pitch))
                .is_some_and(Option::is_some)
        {
            Ok(())
        } else {
            Err(DeviceContractError::InvalidParameterValue)
        }
    }
    pub(super) fn prepare(
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Self, DeviceContractError> {
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
        Ok(Self {
            config: Config {
                identity,
                tuning: *context.tuning,
                sample_rate,
                steps,
                base,
                full,
                authored_phases: settings.oscillators.map(|oscillator| oscillator[2]),
                envelope: PreparedEnvelope {
                    attack: frames(envelope.attack_seconds),
                    decay: frames(envelope.decay_seconds),
                    sustain: envelope.sustain,
                    release: frames(envelope.release_seconds),
                },
                table: crate::shared_sine_table(),
                pressure: None,
                pressure_source: None,
                smoothing: ((u64::from(sample_rate) * 5 + 500) / 1000) as u32,
            },
        })
    }
}
#[derive(Clone, Copy)]
struct PressureRoute {
    index: usize,
    route: PreparedPressureRoute,
    required: [bool; 4],
}

#[derive(Clone, Copy)]
struct Config {
    identity: [u8; PAYLOAD_BYTES],
    tuning: hex_lattice::TuningTable,
    sample_rate: u32,
    steps: [Option<[u64; 4]>; 128],
    base: PreparedFmConfiguration,
    full: PreparedFmConfiguration,
    authored_phases: [f32; 4],
    envelope: PreparedEnvelope,
    table: &'static crate::SineTable,
    smoothing: u32,
    pressure: Option<PressureRoute>,
    pressure_source: Option<hex_lattice_contract::ModulationSourceId>,
}
impl Config {
    fn accepts_note_pressure(&self) -> bool {
        self.pressure_source.is_some() || self.pressure.is_some()
    }
    fn same_pressure_source(&self, other: &Self) -> bool {
        self.pressure_source == other.pressure_source
            && self.accepts_note_pressure() == other.accepts_note_pressure()
    }
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
#[derive(Clone, Copy)]
pub(super) struct Factory {
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
    release_ceiling: u64,
    pressure: u8,
}
impl Voice {
    fn advance(&mut self, frames: u64) {
        if self.id == 0 {
            return;
        }
        self.age = self.age.saturating_add(frames);
        if self
            .gate
            .is_some_and(|gate| self.age.saturating_sub(gate) >= self.release_ceiling)
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
    fn sample(&mut self, target: f64, frames: u32, immediate: bool) -> f64 {
        if immediate {
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
    levels: [f64; 4],
    phase_targets: [f32; 4],
    effective: PreparedFmConfiguration,
    unclamped_pressure_target: Option<(ParameterId, f64)>,
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
            levels: config.full.levels,
            phase_targets: config.authored_phases,
            effective: config.base,
            unclamped_pressure_target: None,
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
                    release_ceiling: config.envelope.release,
                    pressure: 0,
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = voices.iter_mut().find(|voice| voice.id == occurrence_id) {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                    voice.advance(0);
                }
            }
            NativeInstrumentEvent::NotePressure {
                occurrence_id,
                value,
                ..
            } => {
                if !config.accepts_note_pressure() {
                    return Err(Error::UnsupportedNotePressure);
                }
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.id == occurrence_id.get() && voice.gate.is_none())
                    .ok_or(Error::InvalidEventBatch)?;
                voice.pressure = value;
            }
            NativeInstrumentEvent::Expression { .. } => {}
        }
        Ok(())
    }
    fn set_controls(&mut self, levels: [f64; 4], carriers: [f64; 4]) {
        self.levels = levels;
        self.effective = self.config.full;
        self.effective.levels = levels;
        self.effective.routing.carriers = carriers;
        let audible = audible_operator_ancestors(&self.effective.levels, &self.effective.routing);
        for (index, audible) in audible.into_iter().enumerate() {
            if !audible {
                self.effective.levels[index] = 0.0;
            }
        }
    }
    fn active_slots(&self, slots: &mut [usize; VOICES]) -> usize {
        let mut count = 0;
        for (slot, (index, _)) in slots.iter_mut().zip(
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.id != 0),
        ) {
            *slot = index;
            count += 1;
        }
        count
    }
    fn inherit_voices(&mut self, previous: &Self) {
        self.voices = previous.voices;
        self.unclamped_pressure_target = None;
        if self.config.same_pressure_source(&previous.config) {
            self.unclamped_pressure_target =
                previous.unclamped_pressure_target.filter(|(id, _)| {
                    self.config
                        .pressure
                        .is_some_and(|route| route.route.descriptor().id == *id)
                });
        } else {
            for voice in &mut self.voices {
                voice.pressure = 0;
            }
        }
    }
    fn has_steady_base_carriers(&self) -> bool {
        self.controls
            .iter()
            .zip(self.config.base.routing.carriers)
            .zip(self.effective.routing.carriers)
            .all(|((control, base), effective)| {
                control.remaining == 0
                    && control.current.to_bits() == base.to_bits()
                    && control.target.to_bits() == base.to_bits()
                    && effective.to_bits() == base.to_bits()
            })
    }
    fn render(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
    ) {
        if let Some(pressure) = self.config.pressure {
            self.render_block::<true>(events, frames, gains, values, Some(pressure));
        } else {
            self.render_block::<false>(events, frames, gains, values, None);
        }
    }

    fn render_block<const PRESSURE: bool>(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        values: Option<&dyn NativeParameterAutomation>,
        pressure: Option<PressureRoute>,
    ) {
        let pressure = if PRESSURE { pressure } else { None };
        let mut next_event = 0;
        let mut slots = [0; VOICES];
        let mut active_count = self.active_slots(&mut slots);
        let steady_base = values.is_none()
            && self.levels == self.config.full.levels
            && self.has_steady_base_carriers();
        let level_lanes = LEVELS.map(|descriptor| {
            values
                .is_some_and(|values| values.parameter_ids().binary_search(&descriptor.id).is_ok())
        });
        let level_values = values.filter(|_| level_lanes.contains(&true));
        let phase_values = values.filter(|values| {
            PHASES
                .iter()
                .any(|descriptor| values.parameter_ids().binary_search(&descriptor.id).is_ok())
        });
        if phase_values.is_none() {
            self.phase_targets = self.config.authored_phases;
            self.effective.phases = self.config.full.phases;
        }
        let modulated = CARRIERS.map(|descriptor| {
            values.is_some_and(|values| values.has_sample_rate_modulation(descriptor.id))
        });
        for (offset, frame) in frames.iter_mut().enumerate() {
            if !steady_base {
                let carriers = std::array::from_fn(|index| {
                    let target = match values
                        .and_then(|values| values.value_at(CARRIERS[index].id, offset))
                    {
                        Some(ParameterValue::Float(value)) => f64::from(value),
                        _ => self.config.base.routing.carriers[index],
                    };
                    self.controls[index].sample(
                        target,
                        self.config.smoothing,
                        self.cold || modulated[index],
                    )
                });
                let levels = level_values.map_or(self.config.full.levels, |values| {
                    std::array::from_fn(|index| match values.value_at(LEVELS[index].id, offset) {
                        Some(ParameterValue::Float(value)) => f64::from(value),
                        _ => self.config.full.levels[index],
                    })
                });
                if levels != self.levels || carriers != self.effective.routing.carriers {
                    self.set_controls(levels, carriers);
                }
            }
            if let Some(values) = phase_values {
                self.phase_targets = self.config.authored_phases;
                self.effective.phases = self.config.full.phases;
                for (index, descriptor) in PHASES.into_iter().enumerate() {
                    if let Some(ParameterValue::Float(turns)) =
                        values.value_at(descriptor.id, offset)
                    {
                        self.phase_targets[index] = turns;
                        self.effective.phases[index] = phase_from_turns(f64::from(turns));
                    }
                }
            }
            self.cold = false;
            let first_event = next_event;
            while let Some(event) = events
                .get(next_event)
                .filter(|event| event.frame_offset() == offset)
            {
                // Complete preflight proved this infallible for this exact batch.
                let _ = Self::apply(&self.config, &mut self.voices, *event);
                next_event += 1;
            }
            if next_event != first_event {
                active_count = self.active_slots(&mut slots);
            }
            let unclamped = pressure.and_then(|pressure| {
                let id = pressure.route.descriptor().id;
                values
                    .and_then(|values| values.normalized_before_clamp(id, offset))
                    .map(|value| (id, value))
            });
            if PRESSURE {
                self.unclamped_pressure_target = unclamped;
            }
            let unclamped = unclamped.map(|(_, value)| value);
            let mut sum = 0.0_f64;
            for &slot in &slots[..active_count] {
                // Collected from this fixed array; no caller-supplied index.
                let Some(voice) = self.voices.get_mut(slot) else {
                    continue;
                };
                if voice.id == 0 {
                    continue;
                }
                let Some(steps) = self.config.steps[usize::from(voice.pitch)] else {
                    continue;
                };
                let mut envelope = self.config.envelope;
                envelope.release = voice.release_ceiling;
                let amplitude = envelope.level(voice.age, voice.gate.unwrap_or(u64::MAX))
                    * (f64::from(voice.velocity) / 127.0);
                let sample = if amplitude == 0.0 {
                    0.0
                } else {
                    let override_carrier = pressure.and_then(|pressure| {
                        let base = self.effective.routing.carriers[pressure.index];
                        let canonical = ParameterValue::Float(base as f32);
                        match pressure.route.compose(canonical, unclamped, voice.pressure) {
                            Ok(ParameterValue::Float(value)) if value != base as f32 => {
                                Some((pressure, base, f64::from(value)))
                            }
                            _ => None,
                        }
                    });
                    let output = if voice.phase_offsets == [0; 4] && override_carrier.is_none() {
                        self.effective.routed_output::<false>(
                            self.config.table,
                            voice.age,
                            &steps,
                            [1.0; 4],
                        )
                    } else {
                        let mut configuration = self.effective;
                        if let Some((pressure, base, value)) = override_carrier {
                            // Restore only the ancestors of a newly audible carrier.
                            if base == 0.0 {
                                for (index, required) in pressure.required.into_iter().enumerate() {
                                    if required {
                                        configuration.levels[index] = self.levels[index];
                                    }
                                }
                            }
                            configuration.routing.carriers[pressure.index] = value;
                        }
                        for (phase, offset) in
                            configuration.phases.iter_mut().zip(voice.phase_offsets)
                        {
                            *phase = phase.wrapping_add(offset);
                        }
                        configuration.routed_output::<false>(
                            self.config.table,
                            voice.age,
                            &steps,
                            [1.0; 4],
                        )
                    };
                    (output * amplitude) as f32
                };
                let gain = if gains.is_empty() {
                    1.0
                } else {
                    gains
                        .binary_search_by_key(&voice.id, |gain| gain.occurrence_id())
                        .ok()
                        .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
                };
                sum += f64::from(sample) * gain;
                voice.advance(1);
            }
            frame[0] += sum as f32;
            frame[1] += sum as f32;
        }
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn supports_note_pressure(&self) -> bool {
        self.config.accepts_note_pressure()
    }
    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        &TARGETS
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        modulation_semantics(parameter)
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
        3
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
    fn supports_note_pressure(&self) -> bool {
        self.config.accepts_note_pressure()
    }
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        Some(self)
    }
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
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
                voice.advance((offset - previous) as u64);
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
            self.inherit_voices(previous);
            self.controls = previous.controls;
            self.levels = previous.levels;
            self.phase_targets = previous.phase_targets;
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
            self.inherit_voices(previous);
            for voice in self.voices.iter_mut().filter(|voice| voice.id != 0) {
                voice.release_ceiling = voice.release_ceiling.min(self.config.envelope.release);
                voice.advance(0);
                if voice.id == 0 {
                    continue;
                }
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
            self.set_controls(
                self.config.full.levels,
                previous.controls.map(|control| control.current),
            );
            self.phase_targets = self.config.authored_phases;
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
    fn unclamped_modulation_target(&self) -> Option<(ParameterId, f64)> {
        self.unclamped_pressure_target
    }

    fn copy_targets(&self, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != TARGETS.len() {
            return Err(Error::InvalidParameterAutomation);
        }
        let (oscillators, carriers) = output.split_at_mut(LEVELS.len() + PHASES.len());
        for ((slots, level), phase) in oscillators
            .chunks_exact_mut(2)
            .zip(self.levels)
            .zip(self.phase_targets)
        {
            slots[0] = Some(ParameterValue::Float(level as f32));
            slots[1] = Some(ParameterValue::Float(phase));
        }
        for (slot, control) in carriers.iter_mut().zip(self.controls) {
            *slot = Some(ParameterValue::Float(control.target as f32));
        }
        Ok(())
    }

    fn descriptors(&self) -> &[ParameterDescriptor] {
        &TARGETS
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        modulation_semantics(parameter)
    }
    fn preflight(
        &self,
        _: &[NativeInstrumentEvent],
        frames: usize,
        _: Option<&dyn NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        _: &dyn DeviceResources,
    ) -> Result<(), Error> {
        if let Some(pressure) = self.config.pressure {
            let id = pressure.route.descriptor().id;
            if (0..frames).any(|offset| {
                values
                    .normalized_before_clamp(id, offset)
                    .is_some_and(|v| !v.is_finite())
            }) {
                return Err(Error::InvalidParameterAutomation);
            }
        }
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
