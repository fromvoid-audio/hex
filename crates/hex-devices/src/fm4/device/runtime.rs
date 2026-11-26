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
//! Connection values replace coefficients of authored edges in their existing
//! acyclic order. Pruning uses the authored topology so an edge can cross zero
//! without losing ancestors. Missing values restore the authored coefficient.
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
const EDGES: [ParameterDescriptor; 12] = [
    PARAMETERS[12],
    PARAMETERS[13],
    PARAMETERS[14],
    PARAMETERS[15],
    PARAMETERS[16],
    PARAMETERS[17],
    PARAMETERS[18],
    PARAMETERS[19],
    PARAMETERS[20],
    PARAMETERS[21],
    PARAMETERS[22],
    PARAMETERS[23],
];
const TARGETS: [ParameterDescriptor; 24] = [
    LEVELS[0],
    PHASES[0],
    LEVELS[1],
    PHASES[1],
    LEVELS[2],
    PHASES[2],
    LEVELS[3],
    PHASES[3],
    EDGES[0],
    EDGES[1],
    EDGES[2],
    EDGES[3],
    EDGES[4],
    EDGES[5],
    EDGES[6],
    EDGES[7],
    EDGES[8],
    EDGES[9],
    EDGES[10],
    EDGES[11],
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
    fn modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        if modulation_edge(id)
            .is_some_and(|(source, target)| self.full.routing.modulation[source][target] == 0.0)
        {
            return None;
        }
        modulation_semantics(id)
    }

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
    effective_steps: [u64; 4],
    release_ceiling: u64,
    pressure: u8,
    pitch_offset_cents: i16,
    timbre: u8,
}
impl Voice {
    fn bent_steps(
        base_steps: [u64; 4],
        pitch_offset_cents: i16,
        channel_pitch_bend: i16,
    ) -> [u64; 4] {
        if pitch_offset_cents == 0 && channel_pitch_bend == 0 {
            return base_steps;
        }
        let channel_cents = f64::from(channel_pitch_bend)
            / if channel_pitch_bend < 0 {
                8192.0
            } else {
                8191.0
            }
            * 200.0;
        let total_cents = f64::from(pitch_offset_cents) + channel_cents;
        let ratio = 2.0_f64.powf(total_cents / 1200.0);
        base_steps.map(|step| ((step as f64) * ratio).round().max(1.0) as u64)
    }

    fn retarget_steps(&mut self, base_steps: [u64; 4], channel_pitch_bend: i16) {
        let new_steps = Self::bent_steps(base_steps, self.pitch_offset_cents, channel_pitch_bend);
        if new_steps == self.effective_steps {
            return;
        }
        // Oscillator phase is modulo 2^64: integrate `(old_step - new_step) * age`
        // so pitch-bend and tuning steps preserve exact phase continuity.
        for ((offset, old_step), new_step) in self
            .phase_offsets
            .iter_mut()
            .zip(self.effective_steps)
            .zip(new_steps)
        {
            *offset = offset.wrapping_add(old_step.wrapping_sub(new_step).wrapping_mul(self.age));
        }
        self.effective_steps = new_steps;
    }

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
    edge_values_selected: bool,
    effective: PreparedFmConfiguration,
    unclamped_pressure_target: Option<(ParameterId, f64)>,
    channel_pitch_bend: i16,
    cc_timbre: u8,
    cold: bool,
}

mod render;
mod traits;

#[cfg(test)]
mod tests;
