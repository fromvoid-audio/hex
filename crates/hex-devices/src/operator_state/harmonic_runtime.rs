//! Fully owned custom spectra and independent transactional voice histories.
//! The current variable-size instrument contract charges the entire twin.
use super::*;
use crate::operator_waveform::{OperatorWaveformBank, harmonic_cap_index, prepared_bank};
use hex_lattice::{
    InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime, NativeInstrumentClock,
    NativeInstrumentEvent, NativeInstrumentRuntimeError as Error,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};
use std::any::Any;

mod filter;
mod modulation;
mod quality;
mod voices;
use voices::Ledger;
mod spectrum;
pub(super) use filter::Trajectory as FilterTrajectory;
pub(super) use spectrum::Spectrum;
use spectrum::{HarmonicPhaseTable, harmonic_phase_table};
const VOICES: usize = 64;

#[derive(Clone)]
pub(super) struct Factory {
    pub configuration: PreparedOperatorConfiguration,
    // Kept out of the small legacy configuration, which callback parameter
    // interpolation copies. This fixed owner is charged by the wide runtime.
    pub tuning: Option<hex_lattice::TuningTable>,
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub settings: OperatorSettings,
    target: Spectrum,
    table: &'static SineTable,
    bank: &'static OperatorWaveformBank,
    phase_table: &'static HarmonicPhaseTable,
    lfo_table: &'static quality::LfoTable,
    rate: u32,
    parameters: [f64; 12],
    filter: FilterTrajectory,
    companion_active: bool,
    // Identical authored machine laws start equal and receive the same
    // per-voice age, clock observations and note-off. Active history may only
    // transfer across compatible companion and envelope laws. Per-operator
    // machine events would require invalidating this proof.
    uniform_companion: bool,
    companion_sources: [u8; 4],
    partial_companion: bool,
    shared_loop_timing: bool,
    uniform_envelopes: bool,
}
impl Factory {
    #[cfg(test)]
    pub fn new(
        settings: OperatorSettings,
        configuration: PreparedOperatorConfiguration,
        table: &'static SineTable,
        rate: u32,
    ) -> Result<Self, DeviceContractError> {
        Self::new_with_tuning(settings, configuration, table, rate, None)
    }
    pub fn new_with_tuning(
        settings: OperatorSettings,
        configuration: PreparedOperatorConfiguration,
        table: &'static SineTable,
        rate: u32,
        tuning: Option<hex_lattice::TuningTable>,
    ) -> Result<Self, DeviceContractError> {
        let state = settings.to_state()?;
        let parameters = parameter_words(
            configuration
                .feedback_parameters(settings.effective_feedback().map(f64::from))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?,
        );
        // Each alias points backward to the first bit-identical machine law.
        let mut companion_sources = [0, 1, 2, 3];
        for (operator, source) in companion_sources.iter_mut().enumerate().skip(1) {
            for previous in 0..operator {
                if settings.companion.words[operator] == settings.companion.words[previous]
                    && (0..ENVELOPE_FIELDS).all(|field| {
                        settings.fields[CORE_FIELDS + operator * ENVELOPE_FIELDS + field].to_bits()
                            == settings.fields[CORE_FIELDS + previous * ENVELOPE_FIELDS + field]
                                .to_bits()
                    })
                {
                    *source = previous as u8;
                    break;
                }
            }
        }
        let uniform_companion = companion_sources == [0, 0, 0, 0];
        let partial_companion = !uniform_companion && companion_sources != [0, 1, 2, 3];
        // Matching mode, velocity time scaling, loop return, attack and decay
        // imply equal prepared timing for every note velocity at this rate.
        let shared_loop_timing = !uniform_companion
            && !partial_companion
            && (0..4).all(|i| {
                settings.companion.words[i][5] == 1
                    && [3, 6]
                        .into_iter()
                        .all(|j| settings.companion.words[i][j] == settings.companion.words[0][j])
                    && [3, 4].into_iter().all(|j| {
                        settings.fields[CORE_FIELDS + i * ENVELOPE_FIELDS + j].to_bits()
                            == settings.fields[CORE_FIELDS + j].to_bits()
                    })
            });
        Ok(Self {
            uniform_envelopes: configuration.has_uniform_envelopes(),
            configuration,
            tuning,
            base: state.payload()[..ENABLE_PAYLOAD_BYTES]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
            target: Spectrum::prepare(&settings),
            filter: if settings.global.active {
                FilterTrajectory::oversampled(settings.filter, rate)?
            } else {
                FilterTrajectory::new(settings.filter, rate)?
            },
            companion_active: settings.companion.requires_runtime(),
            uniform_companion,
            companion_sources,
            partial_companion,
            shared_loop_timing,
            settings,
            table,
            phase_table: harmonic_phase_table()?,
            lfo_table: quality::lfo_table(),
            bank: prepared_bank().map_err(|_| DeviceContractError::RuntimeAllocation)?,
            rate,
            parameters,
        })
    }
    fn prepare_voice(
        &self,
        pitch: u8,
        velocity: u8,
        origin: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<Voice, Error> {
        let note = if self.settings.lfo.active() {
            self.configuration
                .prepare_lfo_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        } else {
            self.configuration
                .prepare_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        }
        .map_err(|_| Error::InvalidEventBatch)?;
        let settings = self.settings;
        let machines = if settings.companion.requires_runtime() {
            let envelopes = settings
                .companion
                .envelopes(&settings.fields, velocity)
                .map_err(|_| Error::InvalidEventBatch)?;
            let machines = std::array::from_fn(|i| {
                let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
                companion::Machine::prepare(
                    settings.companion,
                    i,
                    envelopes[i],
                    std::array::from_fn(|j| f64::from(settings.fields[base + j])),
                    self.rate,
                    beat,
                    running,
                )
                .map(Some)
            });
            let [a, b, c, d] = machines;
            [a?, b?, c?, d?]
        } else {
            [None; 4]
        };
        let phases = std::array::from_fn(|i| {
            if settings.companion.words[i][0] == 0 {
                PreparedOperatorConfiguration::lfo_steps(&note)[i].wrapping_mul(origin)
            } else {
                0
            }
        });
        Ok(Voice {
            note,
            phases,
            machines,
            ..Voice::default()
        })
    }
    fn capacity(&self) -> usize {
        if self.settings.global.active {
            return 64;
        }
        if self.settings.lfo.active() || self.settings.effective_feedback() != [0.0; 4] {
            16
        } else {
            64
        }
    }
    fn retuned_note(&self, source: PreparedOperatorNote) -> Result<PreparedOperatorNote, Error> {
        let (pitch, velocity) = source.midi_origin();
        if self.settings.lfo.active() {
            self.configuration
                .prepare_lfo_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        } else {
            self.configuration
                .prepare_note_with_tuning(pitch, velocity, self.tuning.as_ref())
        }
        .map_err(|_| Error::InvalidEventBatch)
    }
    fn compatible(&self, other: &Self) -> bool {
        self.rate == other.rate
            && self.settings.companion == other.settings.companion
            && compatible_base(&self.base, &other.base, self.settings.global.active)
    }
    fn parameters(&self) -> [f64; 12] {
        self.parameters
    }
}
fn compatible_base(
    a: &[u8; ENABLE_PAYLOAD_BYTES],
    b: &[u8; ENABLE_PAYLOAD_BYTES],
    allow_routing: bool,
) -> bool {
    a.iter().zip(b).enumerate().all(|(i, (a, b))| {
        let levels = [4..8, 16..20, 28..32, 40..44]
            .iter()
            .any(|r| r.contains(&i));
        let carriers = (112..128).contains(&i);
        let waveforms = (LEGACY_PAYLOAD_BYTES..WAVEFORM_PAYLOAD_BYTES).contains(&i);
        let feedback = (VELOCITY_PAYLOAD_BYTES..FEEDBACK_PAYLOAD_BYTES).contains(&i);
        let enabled = (LFO_PAYLOAD_BYTES..ENABLE_PAYLOAD_BYTES).contains(&i);
        let lfo = (FEEDBACK_PAYLOAD_BYTES..LFO_PAYLOAD_BYTES).contains(&i);
        let routing = allow_routing && (48..112).contains(&i);
        // Keytrack's fixed endpoint mirrors the legacy fixed-mode word. The
        // target prepares all new phase steps before preserving voice history.
        let pitch_mode = (WAVEFORM_PAYLOAD_BYTES..TUNING_PAYLOAD_BYTES).contains(&i)
            && (i - WAVEFORM_PAYLOAD_BYTES) % 12 < 4;
        routing
            || levels
            || carriers
            || waveforms
            || feedback
            || enabled
            || lfo
            || pitch_mode
            || a == b
    })
}
// Keep the partially shared path out of the distinct-envelope render path.
#[inline(never)]
#[cold]
fn partial_levels(
    machines: &[Option<companion::Machine>; 4],
    sources: [u8; 4],
    age: u64,
) -> [f64; 4] {
    let [a, b, c, d] = machines;
    let a = a.as_ref().map_or(0.0, |machine| machine.level(age));
    let b = if sources[1] == 0 {
        a
    } else {
        b.as_ref().map_or(0.0, |machine| machine.level(age))
    };
    let c = match sources[2] {
        0 => a,
        1 => b,
        _ => c.as_ref().map_or(0.0, |machine| machine.level(age)),
    };
    let d = match sources[3] {
        0 => a,
        1 => b,
        2 => c,
        _ => d.as_ref().map_or(0.0, |machine| machine.level(age)),
    };
    [a, b, c, d]
}
#[derive(Clone, Copy, Default)]
pub(super) struct Voice {
    pub note: PreparedOperatorNote,
    pub phases: [u64; 4],
    pub previous: [f64; 4],
    pub age: u64,
    pub gate: Option<u64>,
    pub id: u64,
    pub sequence: u64,
    pub decimator: quality::Decimator,
    pub effective: super::modulation::Effective,
    pub modulated_filter: Option<FilterTrajectory>,
    pub envelope_age: f64,
    pub envelope_gate: Option<u64>,
    pub drain: u32,
    pub pitch_steps: [u64; 4],
    pub pan_gains: [f32; 2],
    pub lfo_rate_multiplier: f64,
    pub filters: [crate::operator_filter::Carrier; 4],
    pub lfo_fraction: f64,
    pub lfo_cycle: u64,
    pub machines: [Option<companion::Machine>; 4],
}
#[derive(Clone, Copy)]
pub(super) struct ParameterRamp {
    pub source: [f64; 12],
    pub elapsed: u32,
    pub frames: u32,
}
impl ParameterRamp {
    fn at(self, target: [f64; 12]) -> [f64; 12] {
        let amount = f64::from(self.elapsed.min(self.frames)) / f64::from(self.frames);
        std::array::from_fn(|i| {
            if amount == 1.0 {
                target[i]
            } else {
                self.source[i] + (target[i] - self.source[i]) * amount
            }
        })
    }
}
pub(super) struct Runtime {
    factory: Factory,
    voices: [Voice; VOICES],
    source: Spectrum,
    elapsed: u32,
    frames: u32,
    parameters: Option<ParameterRamp>,
    filter: FilterTrajectory,
    phase: (f64, u64),
    local_frame: u64,
    ledger: Ledger,
    expression: [f32; 3],
}
// Admission needs identity, age and completion bounds, not oscillator/filter history.
// Keeping the proof separate prevents full 64-voice machine banks on callback stacks.
#[derive(Clone, Copy, Default)]
struct EventVoice {
    id: u64,
    age: u64,
    gate: Option<u64>,
    completion: Option<(u64, u64)>,
    pitch: u8,
    sequence: u64,
    velocity: u8,
    active: bool,
    envelope_age: f64,
    envelope_gate: Option<u64>,
    time_rate: f64,
    drain: u32,
}
impl EventVoice {
    fn read(voice: &Voice, factory: &Factory) -> Self {
        let completion = if factory.companion_active {
            voice
                .machines
                .iter()
                .try_fold((0, 0), |(relative, absolute), machine| {
                    let (r, a) = machine.as_ref()?.completion_bounds();
                    Some((relative.max(r), absolute.max(a)))
                })
        } else {
            Some((factory.configuration.release_frames(), 0))
        };
        Self {
            id: voice.id,
            age: voice.age,
            gate: voice.gate,
            completion,
            pitch: voice.note.midi_origin().0,
            sequence: voice.sequence,
            velocity: voice.note.midi_origin().1,
            active: factory.settings.global.active,
            envelope_age: voice.envelope_age,
            envelope_gate: voice.envelope_gate,
            time_rate: voice.effective.time_rate,
            drain: voice.drain,
        }
    }
    fn envelope_finished(&self) -> bool {
        self.envelope_gate
            .zip(self.completion)
            .is_some_and(|(gate, (relative, absolute))| {
                self.envelope_age as u64 >= gate.saturating_add(relative).max(absolute)
            })
    }
    fn expire(&mut self) {
        if self.active {
            if self.id != 0 && self.drain >= quality::TAIL as u32 {
                *self = Self::default();
            }
            return;
        }
        if self.id != 0
            && self
                .gate
                .zip(self.completion)
                .is_some_and(|(gate, (relative, absolute))| {
                    self.age >= gate.saturating_add(relative).max(absolute)
                })
        {
            *self = Self::default();
        }
    }
}
struct EventProof {
    expression: [f32; 3],
    ledger: Ledger,
    voices: [EventVoice; VOICES],
    local_frame: u64,
}
impl EventProof {
    fn new(runtime: &Runtime) -> Self {
        Self {
            ledger: runtime.ledger,
            expression: runtime.expression,
            voices: std::array::from_fn(|i| EventVoice::read(&runtime.voices[i], &runtime.factory)),
            local_frame: runtime.local_frame,
        }
    }
    fn advance(&mut self, frames: usize) -> Result<(), Error> {
        self.local_frame = self
            .local_frame
            .checked_add(frames as u64)
            .ok_or(Error::InvalidEventBatch)?;
        for voice in &mut self.voices {
            if voice.id != 0 {
                if voice.active {
                    for _ in 0..frames {
                        voice.age = voice.age.checked_add(1).ok_or(Error::InvalidEventBatch)?;
                        voice.envelope_age =
                            (voice.envelope_age + voice.time_rate).min(u64::MAX as f64);
                        if voice.envelope_finished() {
                            voice.drain = voice.drain.saturating_add(1);
                        } else {
                            voice.drain = 0;
                        }
                        voice.expire();
                        if voice.id == 0 {
                            break;
                        }
                    }
                } else {
                    voice.age = voice
                        .age
                        .checked_add(frames as u64)
                        .ok_or(Error::InvalidEventBatch)?;
                }
                voice.expire();
            }
        }
        Ok(())
    }
    fn apply(&mut self, factory: &Factory, event: NativeInstrumentEvent) -> Result<(), Error> {
        if factory.settings.global.active {
            return self.apply_global(factory, event);
        }
        match event {
            NativeInstrumentEvent::Expression { .. } => return Err(Error::InvalidEventBatch),
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0 || self.voices.iter().any(|v| v.id == occurrence_id) {
                    return Err(Error::InvalidEventBatch);
                }
                // Musical clock validation remains separate; this proves all note
                // preparation plus identity/capacity without mutating live history.
                let prepared = factory.prepare_voice(
                    pitch,
                    velocity,
                    self.local_frame,
                    Some(0),
                    Some(false),
                )?;
                let voice = self.voices[..factory.capacity()]
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = EventVoice {
                    id: occurrence_id,
                    ..EventVoice::read(&prepared, factory)
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                let voice = self
                    .voices
                    .iter_mut()
                    .find(|v| v.id == occurrence_id)
                    .ok_or(Error::InvalidEventBatch)?;
                if voice.gate.is_some() {
                    return Err(Error::InvalidEventBatch);
                }
                voice.gate = Some(voice.age);
                voice.expire();
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct Target(u64);
impl PreparedInstrumentHistoryRetarget for Target {
    fn allows_runtime_versions(&self, source: u32, target: u32) -> bool {
        target == 3 && matches!(source, 1..=3)
    }
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        3
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        if self.settings.global.active {
            259
        } else {
            256
        }
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn max_active_occurrences(&self) -> usize {
        // Stolen held notes can resume in mono mode; their identities remain
        // live independently of the audible voice count.
        if self.settings.global.active {
            VOICES
        } else {
            self.capacity()
        }
    }
    fn max_release_tail_frames(&self) -> u64 {
        if self.settings.companion.requires_runtime() {
            140 * u64::from(self.rate)
        } else {
            self.configuration.release_frames()
        }
        .saturating_mul(if self.settings.global.active { 4 } else { 1 })
        .saturating_add(if self.settings.global.active {
            quality::TAIL
        } else {
            0
        })
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            factory: self.clone(),
            voices: [Voice::default(); VOICES],
            source: self.target,
            elapsed: 0,
            frames: 1,
            parameters: None,
            filter: self.filter,
            phase: (0.0, 0),
            local_frame: 0,
            ledger: Ledger::default(),
            expression: [0.0; 3],
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Target(self.configuration.configuration_id()),
        )))
    }
}
impl Runtime {
    fn spectrum(&self) -> Spectrum {
        Spectrum::at(
            self.source,
            self.factory.target,
            f64::from(self.elapsed.min(self.frames)) / f64::from(self.frames),
        )
    }
    fn current_parameters(&self) -> [f64; 12] {
        self.parameters.map_or_else(
            || self.factory.parameters(),
            |r| r.at(self.factory.parameters()),
        )
    }
    fn expire(&mut self) {
        let release = self.factory.configuration.release_frames();
        for voice in &mut self.voices {
            if voice.id == 0 || voice.gate.is_none() {
                continue;
            }
            let finished = if self.factory.settings.global.active {
                voice.drain >= quality::TAIL as u32
            } else if self.factory.companion_active {
                voice.machines.iter().all(|machine| {
                    machine
                        .as_ref()
                        .is_some_and(|machine| machine.finished(voice.age))
                })
            } else {
                voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(release))
            };
            if finished {
                *voice = Voice::default();
            }
        }
    }
    fn apply(
        &mut self,
        event: NativeInstrumentEvent,
        origin: u64,
        beat: Option<u64>,
        running: Option<bool>,
    ) -> Result<(), Error> {
        if self.factory.settings.global.active {
            return self.apply_global(event, origin, beat, running);
        }
        match event {
            NativeInstrumentEvent::Expression { .. } => return Err(Error::InvalidEventBatch),
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0 || self.voices.iter().any(|v| v.id == occurrence_id) {
                    return Err(Error::InvalidEventBatch);
                }
                let prepared = self
                    .factory
                    .prepare_voice(pitch, velocity, origin, beat, running)?;
                let capacity = self.factory.capacity();
                let voice = self.voices[..capacity]
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = Voice {
                    id: occurrence_id,
                    ..prepared
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                let voice = self
                    .voices
                    .iter_mut()
                    .find(|v| v.id == occurrence_id)
                    .ok_or(Error::InvalidEventBatch)?;
                if voice.gate.is_some() {
                    return Err(Error::InvalidEventBatch);
                }
                voice.gate = Some(voice.age);
                for machine in voice.machines.iter_mut().flatten() {
                    machine.note_off(voice.age);
                }
                self.expire();
            }
        }
        Ok(())
    }
}
impl MutableInstrumentRuntime for Runtime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, hex_lattice::NativeInstrumentRuntimeError> {
        super::occurrence_gain::active_ids(
            self.voices
                .iter()
                .map(|v| v.id)
                .chain(self.ledger.held.iter().map(|v| v.id)),
            output,
        )
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        super::occurrence_gain::preflight(
            self.voices
                .iter()
                .map(|v| v.id)
                .chain(self.ledger.held.iter().map(|v| v.id)),
            events,
            gains,
        )
    }

    fn can_import_analytic_history(
        &self,
        history: &hex_lattice::AnalyticInstrumentHistory<'_>,
    ) -> bool {
        let Some(provider) = history.source.analytic_continuation() else {
            return false;
        };
        let Some(source) = provider.as_any().downcast_ref::<PreparedOperatorContext>() else {
            return false;
        };
        let Ok(base) = <&[u8; ENABLE_PAYLOAD_BYTES]>::try_from(
            source
                .authored_state
                .payload()
                .get(..ENABLE_PAYLOAD_BYTES)
                .unwrap_or(&[]),
        ) else {
            return false;
        };
        if self.factory.rate != source.configuration.sample_rate()
            || (!history.voices.is_empty() && self.factory.settings.companion != source.companion)
            || !compatible_base(
                &self.factory.base,
                base,
                self.factory.settings.global.active,
            )
            || !std::ptr::eq(self.factory.table, source.sine_table)
            || history.voices.len() > self.factory.capacity()
        {
            return false;
        }
        let parameters = if let Some(transition) = history.transition {
            provider.parameters_at(history.source_id, &transition, history.performance_frame)
        } else {
            Ok(provider.endpoint(history.source_id))
        };
        if parameters
            .and_then(|p| source.continuation_levels(history.source_id, &p))
            .is_err()
        {
            return false;
        }
        history.voices.iter().enumerate().all(|(index, voice)| {
            voice.occurrence_id != 0
                && !history.voices[..index]
                    .iter()
                    .any(|other| other.occurrence_id == voice.occurrence_id)
                && voice.gate_frame.is_none_or(|gate| {
                    gate <= voice.age_frames
                        && gate
                            .checked_add(voice.note.release_frames())
                            .is_some_and(|end| end > voice.age_frames)
                })
                && source.continuation_note(&voice.note).is_ok_and(|note| {
                    self.factory.retuned_note(note).is_ok_and(|target| {
                        !self.factory.settings.lfo.active()
                            || PreparedOperatorConfiguration::lfo_steps(&target)
                                .iter()
                                .all(|step| *step < (1u64 << 62))
                    })
                })
        })
    }
    fn import_analytic_history(&mut self, history: &hex_lattice::AnalyticInstrumentHistory<'_>) {
        if !self.can_import_analytic_history(history) {
            return;
        }
        let Some(provider) = history.source.analytic_continuation() else {
            return;
        };
        let Some(source) = provider.as_any().downcast_ref::<PreparedOperatorContext>() else {
            return;
        };
        let Ok(base) = <&[u8; ENABLE_PAYLOAD_BYTES]>::try_from(
            source
                .authored_state
                .payload()
                .get(..ENABLE_PAYLOAD_BYTES)
                .unwrap_or(&[]),
        ) else {
            return;
        };
        let Some(spectrum) = builtin_spectrum(base) else {
            return;
        };
        self.voices.fill(Voice::default());
        self.local_frame = history.performance_frame;
        for (target, voice) in self.voices.iter_mut().zip(history.voices) {
            let Ok(source_note) = source.continuation_note(&voice.note) else {
                return;
            };
            // Shared analytic contexts are neutral; custom tables opt into the
            // wide mutable owner before any note is prepared.
            let note = if self.factory.tuning.is_some()
                || self.factory.settings.fine.values() != source.configuration.fine_offsets_hz()
                || self.factory.settings.pitch_keytrack.values()
                    != source.configuration.pitch_keytrack_percent()
            {
                let Ok(note) = self.factory.retuned_note(source_note) else {
                    return;
                };
                note
            } else {
                source_note
            };
            *target = Voice {
                note,
                phases: PreparedOperatorConfiguration::harmonic_initial_phases(
                    &source_note,
                    voice.age_frames,
                ),
                age: voice.age_frames,
                envelope_age: voice.age_frames as f64,
                envelope_gate: voice.gate_frame,
                gate: voice.gate_frame,
                id: voice.occurrence_id,
                ..Voice::default()
            };
        }
        if self.factory.settings.global.active {
            self.seed_ledger();
            self.enforce_polyphony();
        }
        self.filter.from.wet = 0.0;
        self.filter.elapsed = 0;
        self.source = spectrum;
        self.frames = ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32;
        self.elapsed = 0;
        let Ok(p) = source.configuration.feedback_parameters([0.0; 4]) else {
            return;
        };
        let mut target = parameter_words(p);
        target[..4].copy_from_slice(&source.effective_levels);
        self.parameters = None;
        if let Some(transition) = history.transition {
            let Ok(start) = source.continuation_levels(history.source_id, transition.start())
            else {
                return;
            };
            let mut words = target;
            words[..4].copy_from_slice(&start);
            let Ok(elapsed) = transition.elapsed_at(history.performance_frame) else {
                return;
            };
            self.parameters = Some(ParameterRamp {
                source: words,
                elapsed,
                frames: transition.duration(),
            });
        }
        if target != self.factory.parameters() {
            let current = self.parameters.map_or(target, |r| r.at(target));
            self.parameters = Some(ParameterRamp {
                source: current,
                elapsed: 0,
                frames: self.frames,
            });
        }
        if history.voices.is_empty() {
            self.source = self.factory.target;
            self.elapsed = self.frames;
            self.parameters = None;
        }
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > self.factory.max_events_per_block()
            || events
                .iter()
                .filter(|event| !matches!(event, NativeInstrumentEvent::Expression { .. }))
                .count()
                > 256
        {
            return Err(Error::EventCapacityExceeded);
        }
        // With no events and only held voices, no proof voice can expire or
        // affect later admission in this block. Check the same frame and age
        // overflow bounds without replaying each held voice sample by sample.
        if events.is_empty()
            && self.voices.iter().all(|voice| {
                voice.id == 0 || (voice.gate.is_none() && voice.envelope_gate.is_none())
            })
        {
            self.local_frame
                .checked_add(frames as u64)
                .ok_or(Error::InvalidEventBatch)?;
            for voice in self.voices.iter().filter(|voice| voice.id != 0) {
                voice
                    .age
                    .checked_add(frames as u64)
                    .ok_or(Error::InvalidEventBatch)?;
            }
            return Ok(());
        }
        let mut proof = EventProof::new(self);
        let mut offset = 0;
        for event in events {
            let at = match *event {
                NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | NativeInstrumentEvent::NoteOff { frame_offset, .. }
                | NativeInstrumentEvent::Expression { frame_offset, .. } => frame_offset,
            };
            if at < offset || at >= frames {
                return Err(Error::InvalidEventBatch);
            }
            proof.advance(at - offset)?;
            proof.apply(&self.factory, *event)?;
            if self.factory.settings.global.active {
                let mut ids = [0; VOICES];
                super::occurrence_gain::active_ids(
                    proof
                        .voices
                        .iter()
                        .map(|v| v.id)
                        .chain(proof.ledger.held.iter().map(|v| v.id)),
                    &mut ids,
                )
                .map_err(|_| Error::EventCapacityExceeded)?;
            }
            offset = at;
        }
        proof.advance(frames - offset)
    }

    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        let companion = self.factory.settings.companion;
        let beat_required = companion.needs_clock()
            || (self.factory.settings.lfo.active() && self.factory.settings.lfo.sync());
        if companion.needs_running()
            && clock
                .and_then(|clock| clock.transport_is_running())
                .is_none()
        {
            return Err(Error::InvalidEventBatch);
        }
        if beat_required {
            let clock = clock.ok_or(Error::InvalidEventBatch)?;
            let mut before = clock.beat_at_offset(0).ok_or(Error::InvalidEventBatch)?;
            let observations =
                if self.factory.settings.lfo.active() && self.factory.settings.lfo.sync() {
                    frames.checked_add(1).ok_or(Error::InvalidEventBatch)?
                } else {
                    frames
                };
            for offset in 0..observations {
                let next = clock
                    .beat_at_offset(offset)
                    .ok_or(Error::InvalidEventBatch)?;
                // Reserve a full maximum interval plus sixteenth rounding;
                // every machine's next-beat arithmetic is then safe before PCM mutation.
                if next < before || next > u64::MAX - (17u64 << 32) {
                    return Err(Error::InvalidEventBatch);
                }
                before = next;
            }
        }
        self.local_frame
            .checked_add(frames as u64)
            .ok_or(Error::InvalidEventBatch)?;
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock(events, frames, None, _resources);
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, clock, &[], _resources);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut cursor = 0;
        // Events can populate later slots; retirement only removes voices.
        let mut voice_end = self
            .voices
            .iter()
            .rposition(|voice| voice.id != 0)
            .map_or(0, |i| i + 1);
        // Events can create released voices during this block. Once an event
        // occurs, keep checking until the next block rather than maintaining
        // another copy of voice ownership state.
        let mut may_expire = self
            .voices
            .iter()
            .any(|voice| voice.id != 0 && voice.gate.is_some());
        let companion = self.factory.settings.companion;
        let needs_companion_beat = companion.needs_clock();
        let needs_companion_transport = companion.needs_running();
        for (offset, frame) in frames.iter_mut().enumerate() {
            let beat = if needs_companion_beat {
                clock.and_then(|clock| clock.beat_at_offset(offset))
            } else {
                None
            };
            let running = if needs_companion_transport {
                clock.and_then(|clock| clock.transport_is_running())
            } else {
                None
            };
            if self.factory.companion_active {
                for voice in &mut self.voices {
                    if voice.id != 0 {
                        for machine in voice.machines.iter_mut().flatten() {
                            let _ = machine.observe(
                                if self.factory.settings.global.active {
                                    voice.envelope_age as u64
                                } else {
                                    voice.age
                                },
                                beat,
                                running,
                            );
                        }
                    }
                }
            }
            let mut voices_changed = false;
            while let Some(event) = events.get(cursor) {
                let at = match *event {
                    NativeInstrumentEvent::NoteOn { frame_offset, .. }
                    | NativeInstrumentEvent::NoteOff { frame_offset, .. }
                    | NativeInstrumentEvent::Expression { frame_offset, .. } => frame_offset,
                };
                if at != offset {
                    break;
                }
                let origin = clock
                    .and_then(|clock| clock.performance_frame_at_offset(offset))
                    .unwrap_or(self.local_frame);
                let _ = self.apply(*event, origin, beat, running);
                voices_changed = true;
                may_expire = true;
                cursor += 1;
            }
            if voices_changed {
                voice_end = self
                    .voices
                    .iter()
                    .rposition(|voice| voice.id != 0)
                    .map_or(0, |i| i + 1);
            }
            let transition = (self.elapsed < self.frames).then(|| self.spectrum());
            let spectrum = transition.as_ref().unwrap_or(&self.factory.target);
            let parameters = self.current_parameters();
            let filter_frame = self.filter.armed.then(|| self.filter.frame());
            let law = self.factory.settings.lfo;
            let delta = if !law.active() {
                0.0
            } else if law.sync() {
                clock
                    .and_then(|c| c.beat_at_offset(offset).zip(c.beat_at_offset(offset + 1)))
                    .map_or(0.0, |(a, b)| {
                        b.saturating_sub(a) as f64 / 4_294_967_296.0 / law.beats()
                    })
            } else {
                law.rate() / f64::from(self.factory.rate)
            };
            for voice in self.voices.iter_mut().take(voice_end) {
                if voice.id == 0 {
                    continue;
                }
                let phase = if law.retrigger() || self.factory.settings.global.active {
                    (voice.lfo_fraction, voice.lfo_cycle)
                } else {
                    self.phase
                };
                let multiplier = if law.active() {
                    ((if self.factory.settings.global.active {
                        self.factory.lfo_table.sample(
                            law.0[1],
                            phase.0,
                            phase.1,
                            self.factory.settings.global.interpolation,
                        )
                    } else {
                        lfo::waveform(law.0[1], phase.0, phase.1)
                    }) * (law.amount()
                        + if self.factory.settings.global.active {
                            voice.effective.lfo_amount_delta / 100.0
                        } else {
                            0.0
                        })
                    .clamp(-1.0, 1.0))
                    .exp2()
                } else {
                    1.0
                };
                let quality = self.factory.settings.global.active;
                let steps = if quality {
                    voice.pitch_steps
                } else {
                    PreparedOperatorConfiguration::lfo_steps(&voice.note)
                };
                // Envelope age and machines advance only after both quality
                // subframes, so one evaluation serves the whole output frame.
                let age = if quality {
                    voice.envelope_frame()
                } else {
                    voice.age
                };
                let gate = if quality {
                    voice.envelope_gate.unwrap_or(u64::MAX)
                } else {
                    voice.gate.unwrap_or(u64::MAX)
                };
                let amplitudes = if self.factory.companion_active {
                    if self.factory.uniform_companion {
                        [voice.machines[0]
                            .as_ref()
                            .map_or(0.0, |machine| machine.level(age)); 4]
                    } else if self.factory.partial_companion {
                        partial_levels(&voice.machines, self.factory.companion_sources, age)
                    } else {
                        let independent = || {
                            std::array::from_fn(|i| {
                                voice.machines[i]
                                    .as_ref()
                                    .map_or(0.0, |machine| machine.level(age))
                            })
                        };
                        if self.factory.shared_loop_timing {
                            companion::Machine::shared_loop_levels(&voice.machines, age)
                                .unwrap_or_else(independent)
                        } else {
                            independent()
                        }
                    }
                } else if self.factory.uniform_envelopes {
                    self.factory
                        .configuration
                        .uniform_envelope_levels(age, gate)
                } else {
                    self.factory.configuration.envelope_levels(age, gate)
                };
                let mut rendered = 0.0;
                for subframe in 0..if quality { 2 } else { 1 } {
                    let (sample, previous) =
                        self.factory.configuration.sample_with_waveform_sampler(
                            &voice.note,
                            amplitudes,
                            &voice.phases,
                            voice.previous,
                            std::array::from_fn(|i| {
                                (parameters[i]
                                    * if quality {
                                        voice.effective.level_gain[i]
                                    } else {
                                        1.0
                                    })
                                .clamp(0.0, 8.0)
                            }),
                            std::array::from_fn(|i| parameters[4 + i]),
                            std::array::from_fn(|i| parameters[8 + i]),
                            (if quality {
                                voice
                                    .modulated_filter
                                    .filter(|f| f.armed)
                                    .map(|f| f.frame())
                            } else {
                                filter_frame
                            })
                            .map(|frame| (&mut voice.filters, frame)),
                            |operator, phase, _| {
                                let step = steps[operator];
                                spectrum.sample_policy(
                                    operator,
                                    phase,
                                    if law.active() {
                                        if quality {
                                            step
                                        } else {
                                            step.saturating_mul(2)
                                        }
                                    } else if quality {
                                        step / 2
                                    } else {
                                        step
                                    },
                                    voice.age,
                                    PreparedOperatorConfiguration::lfo_steps(&voice.note)[operator],
                                    self.factory.table,
                                    self.factory.bank,
                                    self.factory.phase_table,
                                    !quality || self.factory.settings.global.interpolation,
                                )
                            },
                        );
                    let filtered = if quality {
                        voice.decimator.sample(sample)
                    } else {
                        sample
                    };
                    rendered = if quality && self.factory.settings.global.antialias {
                        filtered
                    } else {
                        sample
                    };
                    voice.previous = previous;
                    for (i, step) in steps.iter().enumerate() {
                        let step = if law.active() && law.selected(i) {
                            (*step as f64 * multiplier).round() as u64
                        } else {
                            *step
                        };
                        let step = if quality {
                            step / 2 + if subframe == 1 { step % 2 } else { 0 }
                        } else {
                            step
                        };
                        voice.phases[i] = voice.phases[i].wrapping_add(step);
                    }
                }
                let pan_gains = if quality { voice.pan_gains } else { [1.0, 1.0] };
                let left = rendered
                    * pan_gains[0]
                    * if quality {
                        voice.effective.gain as f32
                    } else {
                        1.0
                    };
                let right = rendered
                    * pan_gains[1]
                    * if quality {
                        voice.effective.gain as f32
                    } else {
                        1.0
                    };
                frame[0] += super::occurrence_gain::sample(left, voice.id, offset, gains);
                frame[1] += super::occurrence_gain::sample(right, voice.id, offset, gains);
                if law.active() {
                    let next = voice.lfo_fraction
                        + delta
                            * if quality {
                                voice.lfo_rate_multiplier
                            } else {
                                1.0
                            };
                    voice.lfo_cycle = voice.lfo_cycle.wrapping_add(next.floor() as u64);
                    voice.lfo_fraction = next.fract();
                }
                if quality {
                    voice.envelope_age =
                        (voice.envelope_age + voice.effective.time_rate).min(u64::MAX as f64);
                    if let Some(filter) = &mut voice.modulated_filter {
                        filter.advance();
                    }
                }
                voice.age = voice.age.saturating_add(1);
                if quality {
                    if voice.envelope_gate.is_some()
                        && EventVoice::read(voice, &self.factory).envelope_finished()
                    {
                        voice.drain = voice.drain.saturating_add(1);
                    } else {
                        voice.drain = 0;
                    }
                }
            }
            if law.active() {
                let next = self.phase.0 + delta;
                self.phase.1 = self.phase.1.wrapping_add(next.floor() as u64);
                self.phase.0 = next.fract();
            }
            self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
            if let Some(r) = &mut self.parameters {
                r.elapsed += 1;
                if r.elapsed >= r.frames {
                    self.parameters = None;
                }
            }
            self.filter.advance();
            self.local_frame = self.local_frame.saturating_add(1);
            if may_expire {
                self.expire();
            }
        }
    }
    fn reset(&mut self) {
        self.voices.fill(Voice::default());
        self.source = self.factory.target;
        self.elapsed = 1;
        self.frames = 1;
        self.parameters = None;
        self.filter = self.factory.filter;
        self.phase = (0.0, 0);
        self.local_frame = 0;
        self.ledger = Ledger::default();
        self.expression = [0.0; 3];
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
            && (!self.factory.settings.global.active || self.ledger.held.iter().all(|h| h.id == 0))
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|p| {
            self.factory.compatible(&p.factory)
                && self.factory.settings == p.factory.settings
                && self.factory.tuning.as_ref() == p.factory.tuning.as_ref()
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(p) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(p)
        {
            self.voices.copy_from_slice(&p.voices);
            self.source = p.source;
            self.elapsed = p.elapsed;
            self.frames = p.frames;
            self.parameters = p.parameters;
            self.filter = p.filter;
            self.phase = p.phase;
            self.local_frame = p.local_frame;
            self.ledger = p.ledger;
            self.expression = p.expression;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        if let Some(source) = previous.as_any().downcast_ref::<Self>()
            && source.factory.settings.global.active
            && !self.factory.settings.global.active
            && !source.is_quiescent()
        {
            return false;
        }
        let expression = previous
            .as_any()
            .downcast_ref::<Self>()
            .map_or([0.0; 3], |runtime| runtime.expression);
        proof
            .as_any()
            .downcast_ref::<Target>()
            .is_some_and(|t| t.0 == self.factory.configuration.configuration_id())
            && Legacy::read(previous).is_some_and(|p| {
                self.factory.rate == p.rate
                    && (p.voices().all(|voice| voice.id == 0)
                        || self.factory.settings.companion == p.companion)
                    && compatible_base(
                        &self.factory.base,
                        &p.base,
                        self.factory.settings.global.active,
                    )
                    && p.voices().filter(|v| v.id != 0).count() <= self.factory.capacity()
                    && ((p.tuning == self.factory.tuning.as_ref()
                        && p.fine_offsets == self.factory.settings.fine.values()
                        && p.pitch_keytrack == self.factory.settings.pitch_keytrack.values())
                        || p.voices().filter(|v| v.id != 0).all(|v| {
                            self.factory.retuned_note(v.note).is_ok_and(|note| {
                                (!self.factory.settings.lfo.active()
                                    || PreparedOperatorConfiguration::lfo_steps(&note)
                                        .iter()
                                        .all(|step| *step < (1u64 << 62)))
                                    && (!self.factory.settings.global.active
                                        || self
                                            .factory
                                            .effective(note, expression)
                                            .and_then(|e| self.factory.modulated_filter(e))
                                            .is_ok())
                            })
                        }))
                    && (!self.factory.settings.lfo.active()
                        || p.voices().filter(|v| v.id != 0).all(|v| {
                            PreparedOperatorConfiguration::lfo_steps(&v.note)
                                .iter()
                                .all(|step| *step < (1u64 << 62))
                        }))
            })
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if self.can_retarget_history_from(previous, proof)
            && let Some(p) = Legacy::read(previous)
        {
            self.voices.fill(Voice::default());
            for (target, source) in self
                .voices
                .iter_mut()
                .zip(p.voices().filter(|voice| voice.id != 0))
            {
                *target = source;
                if p.tuning != self.factory.tuning.as_ref()
                    || p.fine_offsets != self.factory.settings.fine.values()
                    || p.pitch_keytrack != self.factory.settings.pitch_keytrack.values()
                {
                    let Ok(note) = self.factory.retuned_note(source.note) else {
                        return;
                    };
                    // Phase accumulators, envelopes, feedback, filters and expression
                    // remain at the actual applied boundary; only future increments change.
                    target.note = note;
                }
            }
            if self.factory.settings.global.active {
                if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                    self.ledger = old.ledger;
                    if !old.factory.settings.global.active {
                        self.seed_ledger();
                    }
                } else {
                    self.seed_ledger();
                }
                self.enforce_polyphony();
                if let Some(old) = previous.as_any().downcast_ref::<Self>() {
                    self.expression = old.expression;
                }
                for voice in &mut self.voices {
                    if voice.id != 0 {
                        let _ = voice.update_effective(&self.factory, self.expression);
                    }
                }
            }
            self.phase = p.phase;
            self.local_frame = p.local_frame;
            self.filter = p.filter;
            self.filter.retarget(self.factory.filter);
            if self.factory.target == p.target {
                self.source = p.source;
                self.elapsed = p.elapsed;
                self.frames = p.frames;
            } else {
                self.source = Spectrum::at(
                    p.source,
                    p.target,
                    f64::from(p.elapsed.min(p.frames)) / f64::from(p.frames),
                );
                self.frames = ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32;
                self.elapsed = 0;
            }
            self.parameters = if p.parameters != self.factory.parameters() {
                Some(ParameterRamp {
                    source: p.ramp.map_or(p.parameters, |r| r.at(p.parameters)),
                    elapsed: 0,
                    frames: ((u64::from(self.factory.rate) * 5 + 500) / 1000) as u32,
                })
            } else {
                p.ramp
            };
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Borrow the source bank through validation/retargeting. Legacy runtimes convert
// one voice at a time, so no adapter materializes a 64-machine-bank temporary.
pub(super) trait VoiceHistory {
    fn voice(&self, index: usize) -> Option<Voice>;
}
impl VoiceHistory for Runtime {
    fn voice(&self, index: usize) -> Option<Voice> {
        self.voices.get(index).copied()
    }
}

pub(super) struct Legacy<'a> {
    pub fine_offsets: [f32; 4],
    pub pitch_keytrack: [f32; 4],
    pub tuning: Option<&'a hex_lattice::TuningTable>,
    pub rate: u32,
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub voices: &'a dyn VoiceHistory,
    pub parameters: [f64; 12],
    pub ramp: Option<ParameterRamp>,
    pub phase: (f64, u64),
    pub source: Spectrum,
    pub target: Spectrum,
    pub elapsed: u32,
    pub frames: u32,
    pub filter: FilterTrajectory,
    pub companion: companion::Settings,
    pub local_frame: u64,
}
pub(super) fn builtin_spectrum(base: &[u8; ENABLE_PAYLOAD_BYTES]) -> Option<Spectrum> {
    let mut out = Spectrum {
        coefficients: [[0.0; 64]; 4],
        builtin: [[0.0; crate::operator_waveform::BUILTIN_CHOICE_COUNT]; 4],
        builtin_active: [0; 4],
        custom_active: [false; 4],
    };
    for operator in 0..4 {
        let offset = LEGACY_PAYLOAD_BYTES + operator * 4;
        let shape = u32::from_le_bytes(base[offset..offset + 4].try_into().ok()?);
        OperatorWaveform::from_choice(shape)?;
        let weight = out.builtin[operator].get_mut(shape as usize)?;
        *weight = 1.0;
        out.builtin_active[operator] = 1 << shape;
    }
    Some(out)
}
pub(super) fn parameter_words(p: crate::fm4::OperatorFeedbackParameters) -> [f64; 12] {
    std::array::from_fn(|i| match i {
        0..=3 => p.oscillator_levels()[i],
        4..=7 => p.carrier_weights()[i - 4],
        _ => p.feedback_turns()[i - 8],
    })
}
impl<'a> Legacy<'a> {
    fn voices(&self) -> impl Iterator<Item = Voice> + '_ {
        (0..VOICES).filter_map(|index| self.voices.voice(index))
    }
    fn read(previous: &'a dyn MutableInstrumentRuntime) -> Option<Self> {
        if let Some(p) = previous.as_any().downcast_ref::<Runtime>() {
            return Some(Self {
                fine_offsets: p.factory.settings.fine.values(),
                pitch_keytrack: p.factory.settings.pitch_keytrack.values(),
                tuning: p.factory.tuning.as_ref(),
                rate: p.factory.rate,
                base: p.factory.base,
                voices: p,
                parameters: p.factory.parameters(),
                ramp: p.parameters,
                phase: p.phase,
                source: p.source,
                target: p.factory.target,
                elapsed: p.elapsed,
                frames: p.frames,
                filter: p.filter,
                companion: p.factory.settings.companion,
                local_frame: p.local_frame,
            });
        }
        super::lfo_runtime::export_harmonic_history(previous)
            .or_else(|| super::feedback_runtime::export_harmonic_history(previous))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod global_tests;

#[cfg(test)]
mod fine_tests;
