//! Original pitch LFO and carrier filter; fixed banks allocated only on control.
use super::{
    FEEDBACK_PAYLOAD_BYTES, OperatorSettings,
    lfo::{self, LfoSettings},
};
use crate::{
    SineTable,
    fm4::{PreparedOperatorConfiguration, PreparedOperatorNote},
};
use hex_lattice::{
    InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime, NativeInstrumentClock,
    NativeInstrumentEvent, NativeInstrumentRuntimeError, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

const VOICES: usize = 16;
type Error = NativeInstrumentRuntimeError;

#[derive(Clone)]
pub(super) struct OperatorLfoFactory {
    configuration: PreparedOperatorConfiguration,
    table: &'static SineTable,
    base: [u8; FEEDBACK_PAYLOAD_BYTES],
    lfo: LfoSettings,
    enabled: super::enable::EnableSettings,
    // Validated once on control by `prepare_factory`.
    feedback: [f64; 4],
    release: u64,
    rate: u32,
    filter: Option<crate::operator_filter::Prepared>,
}

impl OperatorLfoFactory {
    fn feedback_parameters(&self) -> crate::fm4::OperatorFeedbackParameters {
        self.configuration
            .validated_feedback_parameters(self.feedback)
    }
}

pub(super) fn prepare_factory(
    settings: &OperatorSettings,
    configuration: PreparedOperatorConfiguration,
    rate: u32,
    table: &'static SineTable,
) -> Result<Option<OperatorLfoFactory>, DeviceContractError> {
    if settings.companion.requires_runtime() {
        return Ok(None);
    }
    if !settings.lfo.active() && !settings.filter.armed {
        return Ok(None);
    }
    let state = settings.to_state()?;
    let feedback = settings.effective_feedback().map(f64::from);
    configuration
        .feedback_parameters(feedback)
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    Ok(Some(OperatorLfoFactory {
        release: configuration.release_frames(),
        configuration,
        table,
        base: state.payload()[..FEEDBACK_PAYLOAD_BYTES]
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?,
        lfo: settings.lfo,
        enabled: settings.enabled,
        feedback,
        rate,
        filter: settings
            .filter
            .armed
            .then(|| settings.filter.prepare(rate))
            .transpose()?,
    }))
}

#[derive(Clone, Copy, Default)]
struct Phase {
    fraction: f64,
    cycle: u64,
}
impl Phase {
    fn advance(&mut self, delta: f64) {
        let next = self.fraction + delta;
        self.cycle = self.cycle.wrapping_add(next.floor() as u64);
        self.fraction = next.fract();
    }
    fn value(self, choice: u32) -> f64 {
        lfo::waveform(choice, self.fraction, self.cycle)
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    note: PreparedOperatorNote,
    phases: [u64; 4],
    previous: [f64; 4],
    age: u64,
    gate: u64,
    id: u64,
    lfo: Phase,
}

struct Runtime<const N: usize = VOICES> {
    factory: OperatorLfoFactory,
    voices: [Voice; N],
    phase: Phase,
    filter: Option<Box<FilterBank<N>>>,
    enable_ramp: Option<EnableRamp>,
}

/// One owner-level ramp, shared by all current notes, including new onsets.
/// A copied twin carries the actual clock; another toggle collapses its scalar
/// boundary without retaining an old factory or allocating a voice ramp.
#[derive(Clone, Copy)]
struct EnableRamp {
    start: [f64; 4],
    elapsed: u32,
    frames: u32,
}

impl EnableRamp {
    fn levels(self, target: [f64; 4]) -> [f64; 4] {
        if self.elapsed >= self.frames {
            return target;
        }
        if self.elapsed == 0 {
            return self.start;
        }
        let amount = f64::from(self.elapsed) / f64::from(self.frames);
        std::array::from_fn(|index| {
            self.start[index] + (target[index] - self.start[index]) * amount
        })
    }
}

#[derive(Clone, Copy)]
struct ParameterRamp {
    start: crate::fm4::OperatorFeedbackParameters,
    target: crate::fm4::OperatorFeedbackParameters,
    elapsed: u32,
    frames: u32,
}
impl ParameterRamp {
    fn value(self) -> crate::fm4::OperatorFeedbackParameters {
        self.start
            .interpolate(self.target, self.elapsed, self.frames)
    }
}
#[derive(Clone, Copy)]
struct FilterBank<const N: usize> {
    parameters: Option<ParameterRamp>,
    carriers: [[crate::operator_filter::Carrier; 4]; N],
    from: crate::operator_filter::Frame,
    target: crate::operator_filter::Frame,
    elapsed: u32,
    frames: u32,
}
impl<const N: usize> FilterBank<N> {
    fn new(p: crate::operator_filter::Prepared) -> Self {
        Self {
            parameters: None,
            carriers: [[crate::operator_filter::Carrier::default(); 4]; N],
            from: p.frame,
            target: p.frame,
            elapsed: 0,
            frames: (p.rate * 5 + 500) / 1000,
        }
    }
    fn frame(&self) -> crate::operator_filter::Frame {
        self.from.interpolate(
            self.target,
            f64::from(self.elapsed) / f64::from(self.frames),
        )
    }
    fn retarget(&mut self, target: crate::operator_filter::Frame) {
        if self.target != target {
            self.from = self.frame();
            self.target = target;
            self.elapsed = 0;
        }
    }
}
impl OperatorLfoFactory {
    pub(super) fn prepare_note_for_runtime(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedOperatorNote, crate::Invalid> {
        if self.lfo.active() {
            self.configuration.prepare_lfo_note(pitch, velocity)
        } else {
            self.configuration.prepare_note(pitch, velocity)
        }
    }
    fn large(&self) -> bool {
        self.filter.is_some() && !self.lfo.active() && self.feedback == [0.0; 4]
    }
    fn make<const N: usize>(&self) -> Runtime<N> {
        Runtime {
            factory: self.clone(),
            voices: [Voice::default(); N],
            phase: Phase::default(),
            filter: self.filter.map(|p| Box::new(FilterBank::new(p))),
            enable_ramp: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Retarget(u64);
impl PreparedInstrumentHistoryRetarget for Retarget {
    fn allows_runtime_versions(&self, source: u32, target: u32) -> bool {
        target == 2 && (source == 1 || source == 2)
    }
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedMutableInstrumentRuntime for OperatorLfoFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn can_import_analytic_context(
        &self,
        source: &dyn hex_lattice::PreparedSharedAnalyticContext,
    ) -> bool {
        self.large()
            && source
                .import_identity()
                .and_then(|p| p.downcast_ref::<super::PreparedOperatorContext>())
                .is_some_and(|p| {
                    self.configuration.identical_to(&p.configuration)
                        && std::ptr::eq(self.table, p.sine_table)
                })
    }
    fn runtime_version(&self) -> u32 {
        2
    }
    fn declared_twin_bytes(&self) -> usize {
        if self.large() {
            size_of::<Runtime<64>>() + size_of::<FilterBank<64>>()
        } else {
            size_of::<Runtime>()
                + if self.filter.is_some() {
                    size_of::<FilterBank<VOICES>>()
                } else {
                    0
                }
        }
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn max_active_occurrences(&self) -> usize {
        if self.large() { 64 } else { VOICES }
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        if self.large() {
            Box::new(self.make::<64>())
        } else {
            Box::new(self.make::<VOICES>())
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Retarget(self.configuration.configuration_id()),
        )))
    }
}

impl<const N: usize> Runtime<N> {
    fn compatible_base(&self, other: &Self) -> bool {
        self.factory.base == other.factory.base
            && self.factory.rate == other.factory.rate
            && std::ptr::eq(self.factory.table, other.factory.table)
    }
    fn current_levels(&self) -> [f64; 4] {
        let target = self.factory.configuration.effective_oscillator_levels();
        self.enable_ramp.map_or(target, |r| r.levels(target))
    }
    fn current_parameters(&self) -> crate::fm4::OperatorFeedbackParameters {
        let p = self
            .filter
            .as_ref()
            .and_then(|b| b.parameters)
            .map_or_else(|| self.factory.feedback_parameters(), ParameterRamp::value);
        if self.enable_ramp.is_some() {
            p.with_oscillator_levels(self.current_levels())
        } else {
            p
        }
    }
    fn copy(&mut self, other: &Self) {
        let target = self.factory.feedback_parameters();
        let old_target = other.factory.feedback_parameters();
        let parameters = if target == old_target {
            if let Some(ramp) = other.enable_ramp {
                Some(ParameterRamp {
                    start: other.current_parameters(),
                    target,
                    elapsed: 0,
                    frames: ramp.frames - ramp.elapsed,
                })
            } else {
                other.filter.as_ref().and_then(|b| b.parameters)
            }
        } else {
            Some(ParameterRamp {
                start: other.current_parameters(),
                target,
                elapsed: 0,
                frames: (self.factory.rate * 5 + 500) / 1000,
            })
        };
        self.voices = other.voices;
        self.phase = other.phase;
        self.enable_ramp = if self.filter.is_none() {
            other.enable_ramp
        } else {
            None
        };
        if let Some(bank) = self.filter.as_deref_mut() {
            if let Some(old) = other.filter.as_deref() {
                *bank = *old;
            } else {
                bank.from = bank.target;
                bank.from.wet = 0.0;
                bank.elapsed = 0;
            }
            bank.parameters = parameters;
            if let Some(target) = self.factory.filter {
                bank.retarget(target.frame);
            }
        }
    }
    fn expire(&mut self) {
        for voice in &mut self.voices {
            if voice.id != 0
                && voice.gate != u64::MAX
                && voice.age >= voice.gate.saturating_add(self.factory.release)
            {
                *voice = Voice::default();
            }
        }
    }
    fn advance_proof(&mut self, frames: usize) -> Result<(), Error> {
        let frames = u64::try_from(frames).map_err(|_| Error::InvalidEventBatch)?;
        for voice in &mut self.voices {
            if voice.id != 0 {
                voice.age = voice
                    .age
                    .checked_add(frames)
                    .ok_or(Error::InvalidEventBatch)?;
            }
        }
        self.expire();
        Ok(())
    }
    fn apply(&mut self, event: NativeInstrumentEvent) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(Error::UnsupportedNotePressure);
            }
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
                let note = self
                    .factory
                    .prepare_note_for_runtime(pitch, velocity)
                    .map_err(|_| Error::InvalidEventBatch)?;
                let index = self
                    .voices
                    .iter()
                    .position(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                let slot = &mut self.voices[index];
                if let Some(bank) = self.filter.as_deref_mut() {
                    bank.carriers[index] = [crate::operator_filter::Carrier::default(); 4];
                }
                *slot = Voice {
                    note,
                    id: occurrence_id,
                    gate: u64::MAX,
                    lfo: if self.factory.lfo.retrigger() {
                        Phase::default()
                    } else {
                        self.phase
                    },
                    ..Voice::default()
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                let voice = self
                    .voices
                    .iter_mut()
                    .find(|v| v.id == occurrence_id && v.id != 0 && v.gate == u64::MAX)
                    .ok_or(Error::InvalidEventBatch)?;
                voice
                    .age
                    .checked_add(self.factory.release)
                    .ok_or(Error::InvalidEventBatch)?;
                voice.gate = voice.age;
                self.expire();
            }
        }
        Ok(())
    }
}

impl<const N: usize> MutableInstrumentRuntime for Runtime<N> {
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
        super::occurrence_gain::active_ids(self.voices.iter().map(|v| v.id), output)
    }
    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[hex_lattice::NativeInstrumentOccurrenceGain],
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        super::occurrence_gain::preflight(self.voices.iter().map(|v| v.id), events, gains)
    }

    fn can_import_analytic(&self, notes: &[hex_lattice::NativeInstrumentAnalyticSeed]) -> bool {
        self.factory.large()
            && self.filter.is_some()
            && notes.len() <= N
            && self.is_quiescent()
            && notes.iter().enumerate().all(|(i, seed)| {
                seed.occurrence_id != 0
                    && !notes[..i]
                        .iter()
                        .any(|other| other.occurrence_id == seed.occurrence_id)
                    && seed.note.release_frames() == self.factory.release
                    && seed.age_frames < u64::MAX
                    && seed.gate_frame.is_none_or(|gate| {
                        gate <= seed.age_frames
                            && gate
                                .checked_add(self.factory.release)
                                .is_some_and(|end| seed.age_frames < end)
                    })
                    && PreparedOperatorNote::from_shared_payload(seed.note.payload()).is_some_and(
                        |note| self.factory.configuration.note_matches_configuration(&note),
                    )
            })
    }
    fn import_analytic(&mut self, notes: &[hex_lattice::NativeInstrumentAnalyticSeed]) {
        if !self.can_import_analytic(notes) {
            return;
        }
        for (voice, seed) in self.voices.iter_mut().zip(notes) {
            if let Some(note) = PreparedOperatorNote::from_shared_payload(seed.note.payload()) {
                *voice = Voice {
                    note,
                    phases: PreparedOperatorConfiguration::lfo_steps(&note)
                        .map(|step| step.wrapping_mul(seed.age_frames)),
                    age: seed.age_frames,
                    gate: seed.gate_frame.unwrap_or(u64::MAX),
                    id: seed.occurrence_id,
                    ..Voice::default()
                };
            }
        }
        if let Some(bank) = self.filter.as_deref_mut() {
            bank.from = bank.target;
            bank.from.wet = 0.0;
            bank.elapsed = 0;
        }
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        if events.len() > 256 {
            return Err(Error::EventCapacityExceeded);
        }
        let mut proof = Self {
            factory: self.factory.clone(),
            voices: self.voices,
            phase: self.phase,
            filter: None,
            enable_ramp: None,
        };
        let mut offset = 0;
        for event in events {
            let next = event.frame_offset();
            if next < offset || next >= frames {
                return Err(Error::InvalidEventBatch);
            }
            proof.advance_proof(next - offset)?;
            proof.apply(*event)?;
            offset = next;
        }
        proof.advance_proof(frames - offset)
    }
    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Error> {
        if !self.factory.lfo.active() || !self.factory.lfo.sync() {
            return Ok(());
        }
        let clock = clock.ok_or(Error::InvalidEventBatch)?;
        let mut previous = clock.beat_at_offset(0).ok_or(Error::InvalidEventBatch)?;
        for offset in 1..=frames {
            let beat = clock
                .beat_at_offset(offset)
                .ok_or(Error::InvalidEventBatch)?;
            if beat < previous {
                return Err(Error::InvalidEventBatch);
            }
            previous = beat;
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock(events, frames, None, &hex_lattice::EMPTY_DEVICE_RESOURCES);
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
        let law = self.factory.lfo;
        let mut event = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            while let Some(next) = events.get(event) {
                let at = next.frame_offset();
                if at != offset {
                    break;
                }
                let _ = self.apply(*next);
                event += 1;
            }
            let delta = if !law.active() {
                0.0
            } else if law.sync() {
                let Some(clock) = clock else {
                    return;
                };
                let (Some(a), Some(b)) = (
                    clock.beat_at_offset(offset),
                    clock.beat_at_offset(offset + 1),
                ) else {
                    return;
                };
                (b.saturating_sub(a)) as f64 / 4_294_967_296.0 / law.beats()
            } else {
                law.rate() / f64::from(self.factory.rate)
            };
            let filter_frame = self.filter.as_ref().map(|bank| bank.frame());
            let parameters = if self.enable_ramp.is_some()
                || self.filter.as_ref().is_some_and(|b| b.parameters.is_some())
            {
                Some(self.current_parameters())
            } else {
                None
            };
            let adjusted =
                parameters.map(|p| self.factory.configuration.with_runtime_parameters(p));
            let configuration = adjusted.as_ref().unwrap_or(&self.factory.configuration);
            let feedback = parameters.map_or(self.factory.feedback, |p| *p.feedback_turns());
            for (index, voice) in self.voices.iter_mut().enumerate() {
                if voice.id == 0 {
                    continue;
                }
                let lfo = if law.retrigger() {
                    voice.lfo
                } else {
                    self.phase
                };
                let multiplier = if law.active() {
                    (lfo.value(law.0[1]) * law.amount()).exp2()
                } else {
                    1.0
                };
                let (sample, previous) = if let (Some(bank), Some(filter)) = (
                    self.filter.as_deref_mut(),
                    filter_frame.filter(|frame| frame.wet != 0.0),
                ) {
                    configuration.sample_filtered_carriers(
                        self.factory.table,
                        &voice.note,
                        voice.age,
                        voice.gate,
                        &voice.phases,
                        feedback,
                        voice.previous,
                        &mut bank.carriers[index],
                        filter,
                    )
                } else {
                    configuration.sample_lfo(
                        self.factory.table,
                        &voice.note,
                        voice.age,
                        voice.gate,
                        &voice.phases,
                        feedback,
                        voice.previous,
                    )
                };
                let sample = super::occurrence_gain::sample(sample, voice.id, offset, gains);
                frame[0] += sample;
                frame[1] += sample;
                voice.previous = previous;
                let steps = PreparedOperatorConfiguration::lfo_steps(&voice.note);
                for (i, step) in steps.iter().enumerate() {
                    let step = if law.active() && law.selected(i) {
                        (*step as f64 * multiplier).round() as u64
                    } else {
                        *step
                    };
                    voice.phases[i] = voice.phases[i].wrapping_add(step);
                }
                voice.lfo.advance(delta);
                voice.age = voice.age.saturating_add(1);
            }
            if let Some(bank) = self.filter.as_deref_mut() {
                bank.elapsed = bank.elapsed.saturating_add(1).min(bank.frames);
            }
            if let Some(bank) = self.filter.as_deref_mut()
                && let Some(ramp) = &mut bank.parameters
            {
                ramp.elapsed += 1;
                if ramp.elapsed >= ramp.frames {
                    bank.parameters = None;
                }
            }
            if let Some(ramp) = &mut self.enable_ramp {
                ramp.elapsed += 1;
                if ramp.elapsed >= ramp.frames {
                    self.enable_ramp = None;
                }
            }
            self.phase.advance(delta);
            self.expire();
        }
    }
    fn reset(&mut self) {
        self.voices = [Voice::default(); N];
        self.phase = Phase::default();
        self.enable_ramp = None;
        if let (Some(bank), Some(p)) = (self.filter.as_deref_mut(), self.factory.filter) {
            *bank = FilterBank::new(p);
        }
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|p| {
            self.compatible_base(p)
                && self.factory.lfo == p.factory.lfo
                && self.factory.filter == p.factory.filter
                && self.factory.enabled == p.factory.enabled
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(p) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(p)
        {
            self.copy(p);
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        if proof
            .as_any()
            .downcast_ref::<Retarget>()
            .is_none_or(|p| p.0 != self.factory.configuration.configuration_id())
        {
            return false;
        }
        if let Some(p) = previous.as_any().downcast_ref::<Self>() {
            return (self.filter.is_some() || (p.filter.is_none() && self.compatible_base(p)))
                && self.factory.rate == p.factory.rate
                && std::ptr::eq(self.factory.table, p.factory.table)
                && self
                    .factory
                    .configuration
                    .retarget_compatible_with(&p.factory.configuration);
        }
        N == VOICES
            && self.filter.is_some()
            && !self.factory.lfo.active()
            && previous
                .as_any()
                .downcast_ref::<super::feedback_runtime::OperatorFeedbackRuntime>()
                .is_some_and(|p| {
                    p.filter_seeds(
                        &self.factory.base,
                        &self.factory.configuration,
                        self.factory.rate,
                        self.factory.table,
                    )
                    .is_some()
                })
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if !self.can_retarget_history_from(previous, proof) {
            return;
        }
        if let Some(p) = previous.as_any().downcast_ref::<Self>() {
            let changed = self.factory.enabled != p.factory.enabled;
            let boundary = p.current_levels();
            self.copy(p);
            if changed && self.filter.is_none() {
                self.enable_ramp = Some(EnableRamp {
                    start: boundary,
                    elapsed: 0,
                    frames: (self.factory.rate * 5 + 500) / 1000,
                });
            }
            return;
        }
        if let Some(old) = previous
            .as_any()
            .downcast_ref::<super::feedback_runtime::OperatorFeedbackRuntime>()
            && let Some(seeds) = old.filter_seeds(
                &self.factory.base,
                &self.factory.configuration,
                self.factory.rate,
                self.factory.table,
            )
        {
            for (voice, seed) in self.voices.iter_mut().zip(seeds) {
                *voice = seed.map_or(Voice::default(), |seed| Voice {
                    note: seed.note,
                    phases: PreparedOperatorConfiguration::lfo_steps(&seed.note)
                        .map(|step| step.wrapping_mul(seed.age)),
                    previous: seed.previous,
                    age: seed.age,
                    gate: seed.gate.unwrap_or(u64::MAX),
                    id: seed.id,
                    lfo: Phase::default(),
                });
            }
            if let Some(bank) = self.filter.as_deref_mut() {
                bank.from = bank.target;
                bank.from.wet = 0.0;
                bank.elapsed = 0;
                bank.parameters =
                    old.filter_parameter_boundary()
                        .map(|(start, frames)| ParameterRamp {
                            start,
                            target: self.factory.feedback_parameters(),
                            elapsed: 0,
                            frames,
                        });
            }
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
            + if self.filter.is_some() {
                size_of::<FilterBank<N>>()
            } else {
                0
            }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
#[path = "lfo_enable_tests.rs"]
mod enable_tests;

impl<const N: usize> super::harmonic_runtime::VoiceHistory for Runtime<N> {
    fn voice(&self, index: usize) -> Option<super::harmonic_runtime::Voice> {
        use super::harmonic_runtime::Voice;
        let old = self.voices.get(index)?;
        Some(Voice {
            note: old.note,
            phases: old.phases,
            previous: old.previous,
            age: old.age,
            gate: (old.gate != u64::MAX).then_some(old.gate),
            id: old.id,
            filters: self
                .filter
                .as_ref()
                .map_or([crate::operator_filter::Carrier::default(); 4], |b| {
                    b.carriers[index]
                }),
            lfo_fraction: old.lfo.fraction,
            lfo_cycle: old.lfo.cycle,
            machines: [None; 4],
            sequence: 0,
            decimator: Default::default(),
            effective: Default::default(),
            modulated_filter: None,
            drain: 0,
            pitch_steps: Default::default(),
            pan_gains: Default::default(),
            lfo_rate_multiplier: 1.0,
            envelope_age: old.age as f64,
            envelope_gate: (old.gate != u64::MAX).then_some(old.gate),
        })
    }
}

pub(super) fn export_harmonic_history(
    runtime: &dyn MutableInstrumentRuntime,
) -> Option<super::harmonic_runtime::Legacy<'_>> {
    if let Some(runtime) = runtime.as_any().downcast_ref::<Runtime<64>>() {
        return export(runtime);
    }
    let runtime = runtime.as_any().downcast_ref::<Runtime>()?;
    export(runtime)
}
fn export<const N: usize>(runtime: &Runtime<N>) -> Option<super::harmonic_runtime::Legacy<'_>> {
    use super::harmonic_runtime::{
        FilterTrajectory, Legacy, ParameterRamp, builtin_spectrum, parameter_words,
    };
    let mut base = [0; super::ENABLE_PAYLOAD_BYTES];
    base[..FEEDBACK_PAYLOAD_BYTES].copy_from_slice(&runtime.factory.base);
    for (i, word) in runtime.factory.lfo.0.iter().enumerate() {
        base[FEEDBACK_PAYLOAD_BYTES + i * 4..FEEDBACK_PAYLOAD_BYTES + i * 4 + 4]
            .copy_from_slice(&word.to_le_bytes());
    }
    for (i, enabled) in runtime.factory.enabled.flags().iter().enumerate() {
        base[super::LFO_PAYLOAD_BYTES + i * 4..super::LFO_PAYLOAD_BYTES + i * 4 + 4]
            .copy_from_slice(&u32::from(*enabled).to_le_bytes());
    }
    let parameters = parameter_words(runtime.factory.feedback_parameters());
    let mut ramp = runtime.enable_ramp.map(|r| ParameterRamp {
        source: std::array::from_fn(|i| if i < 4 { r.start[i] } else { parameters[i] }),
        elapsed: r.elapsed,
        frames: r.frames,
    });
    if let Some(r) = runtime.filter.as_ref().and_then(|b| b.parameters) {
        ramp = Some(ParameterRamp {
            source: parameter_words(r.start),
            elapsed: r.elapsed,
            frames: r.frames,
        });
    }
    let mut filter = FilterTrajectory::inactive(runtime.factory.rate);
    if let Some(bank) = runtime.filter.as_ref() {
        filter = FilterTrajectory {
            from: bank.from,
            target: bank.target,
            elapsed: bank.elapsed,
            frames: bank.frames,
            armed: true,
        };
    }
    let spectrum = builtin_spectrum(&base)?;
    Some(Legacy {
        fine_offsets: runtime.factory.configuration.fine_offsets_hz(),
        pitch_keytrack: runtime.factory.configuration.pitch_keytrack_percent(),
        tuning: None,
        rate: runtime.factory.rate,
        companion: super::companion::Settings::default(),
        local_frame: 0,
        base,
        voices: runtime,
        parameters,
        ramp,
        phase: (runtime.phase.fraction, runtime.phase.cycle),
        source: spectrum,
        target: spectrum,
        elapsed: 1,
        frames: 1,
        filter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    fn factory(sync: bool, waveform: u32, retrigger: bool) -> OperatorLfoFactory {
        let mut state = OPERATOR_METADATA.default_state().unwrap();
        for (id, value) in [
            (29, ParameterValue::Float(1.0)),
            (33, ParameterValue::Float(1.0)),
            (34, ParameterValue::Float(1.0)),
            (35, ParameterValue::Float(1.0)),
            (36, ParameterValue::Float(0.0)),
            (37, ParameterValue::Float(0.001)),
            (38, ParameterValue::Float(0.01)),
            (82, ParameterValue::Bool(true)),
            (83, ParameterValue::Choice(waveform)),
            (84, ParameterValue::Bool(sync)),
            (85, ParameterValue::Float(100.0)),
            (87, ParameterValue::Float(100.0)),
            (88, ParameterValue::Bool(retrigger)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    hex_lattice_contract::ParameterId::from_static(id),
                    value,
                )
                .unwrap();
        }
        for op in 1..=4 {
            state = OPERATOR_METADATA
                .patch_parameter(
                    &state,
                    parameter::envelope(op, parameter::EnvelopeField::Release),
                    ParameterValue::Float(0.01),
                )
                .unwrap();
        }
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_level(1),
                ParameterValue::Float(1.0),
            )
            .unwrap();
        let settings = OperatorSettings::from_state(&state).unwrap();
        prepare_factory(
            &settings,
            settings.prepare_configuration(48_000).unwrap(),
            48_000,
            crate::shared_sine_table(),
        )
        .unwrap()
        .unwrap()
    }

    struct Clock(usize);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            let frame = self.0 + offset;
            // Original two-tempo oracle: 120 bpm until200, then240 bpm.
            Some(
                ((frame.min(200) as u128 * 2 + frame.saturating_sub(200) as u128 * 4)
                    * (1_u128 << 32)
                    / 48_000) as u64,
            )
        }
    }

    fn render(partition: usize, sync: bool, waveform: u32, retrigger: bool) -> Vec<StereoFrame> {
        let factory = factory(sync, waveform, retrigger);
        assert!(
            factory.declared_twin_bytes() * 2 <= 10240,
            "pair {}",
            factory.declared_twin_bytes() * 2
        );
        let mut runtime = factory.create_runtime();
        let mut output = vec![[0.0; 2]; 950];
        for start in (0..950).step_by(partition) {
            let end = (start + partition).min(950);
            let mut events = Vec::new();
            for (at, event) in [
                (
                    23,
                    NativeInstrumentEvent::NoteOn {
                        frame_offset: 0,
                        occurrence_id: 1,
                        pitch: 69,
                        velocity: 127,
                    },
                ),
                (
                    370,
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: 0,
                        occurrence_id: 1,
                    },
                ),
            ] {
                if (start..end).contains(&at) {
                    events.push(match event {
                        NativeInstrumentEvent::NotePressure { .. } => {
                            panic!("unexpected test note pressure")
                        }
                        NativeInstrumentEvent::Expression { expression, .. } => {
                            NativeInstrumentEvent::Expression {
                                frame_offset: at - start,
                                expression,
                            }
                        }
                        NativeInstrumentEvent::NoteOn {
                            occurrence_id,
                            pitch,
                            velocity,
                            ..
                        } => NativeInstrumentEvent::NoteOn {
                            frame_offset: at - start,
                            occurrence_id,
                            pitch,
                            velocity,
                        },
                        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                            NativeInstrumentEvent::NoteOff {
                                frame_offset: at - start,
                                occurrence_id,
                            }
                        }
                    });
                }
            }
            let clock = Clock(start);
            runtime
                .preflight_events(&events, end - start, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .unwrap();
            runtime
                .preflight_clock(
                    Some(&clock),
                    end - start,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                )
                .unwrap();
            runtime.process_block_with_clock(
                &events,
                &mut output[start..end],
                Some(&clock),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
        }
        assert!(runtime.is_quiescent());
        output
    }

    #[test]
    fn independent_pitch_integration_oracle_offsets_tempo_release_and_partitions() {
        for sync in [false, true] {
            for retrigger in [false, true] {
                let output = render(1, sync, 0, retrigger);
                let mut phase = 0.0;
                let mut lfo = if retrigger {
                    0.0
                } else if sync {
                    46.0 / 48_000.0
                } else {
                    23.0 * 100.0 / 48_000.0
                };
                for (at, frame) in output.iter().enumerate().take(850).skip(23) {
                    let age = at - 23;
                    let envelope = if at < 370 {
                        1.0
                    } else {
                        0.00001_f64.powf((at - 370) as f64 / 480.0)
                    };
                    let expected = (std::f64::consts::TAU * phase).sin() * envelope;
                    assert!(
                        (f64::from(frame[0]) - expected).abs() < 3e-6,
                        "at {at} sync {sync} retrigger {retrigger} age {age}"
                    );
                    phase += 440.0 / 48_000.0 * (std::f64::consts::TAU * lfo).sin().exp2();
                    lfo += if sync {
                        if at < 200 {
                            2.0 / 48_000.0
                        } else {
                            4.0 / 48_000.0
                        }
                    } else {
                        100.0 / 48_000.0
                    };
                }
                for partition in [17, 64, 127, 256] {
                    assert_eq!(output, render(partition, sync, 0, retrigger));
                }
            }
        }
        for waveform in 1..=4 {
            assert_eq!(
                render(1, false, waveform, true),
                render(127, false, waveform, true)
            );
        }
    }

    #[test]
    fn rate_depth_and_sample_rate_endpoints_follow_original_pitch_oracle() {
        for sample_rate in [8_000, 44_100, 96_000] {
            for rate in [0.01_f32, 100.0] {
                for amount in [0.0_f32, 100.0] {
                    let mut original = factory(false, 0, true);
                    original.lfo.0[3] = rate.to_bits();
                    original.lfo.0[5] = amount.to_bits();
                    let mut payload = original.base.to_vec();
                    original.lfo.append(&mut payload);
                    let state = hex_lattice_contract::NativeDeviceState::new(
                        super::super::OPERATOR_TYPE_ID,
                        super::super::LFO_STATE_VERSION,
                        payload,
                    )
                    .unwrap();
                    let settings = OperatorSettings::from_state(&state).unwrap();
                    let factory = prepare_factory(
                        &settings,
                        settings.prepare_configuration(sample_rate).unwrap(),
                        sample_rate,
                        crate::shared_sine_table(),
                    )
                    .unwrap()
                    .unwrap();
                    let mut runtime = factory.create_runtime();
                    let events = [NativeInstrumentEvent::NoteOn {
                        frame_offset: 0,
                        occurrence_id: 1,
                        pitch: 69,
                        velocity: 127,
                    }];
                    let mut frames = [[0.0; 2]; 997];
                    runtime
                        .preflight_events(
                            &events,
                            frames.len(),
                            &hex_lattice::EMPTY_DEVICE_RESOURCES,
                        )
                        .unwrap();
                    runtime.process_block(
                        &events,
                        &mut frames,
                        &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    );
                    let mut phase = 0.0;
                    for (at, frame) in frames.iter().enumerate() {
                        let expected = (std::f64::consts::TAU * phase).sin();
                        assert!(
                            (f64::from(frame[0]) - expected).abs() < 3e-6,
                            "rate={rate} amount={amount} Fs={sample_rate} at={at}"
                        );
                        let wave = (std::f64::consts::TAU * at as f64 * f64::from(rate)
                            / f64::from(sample_rate))
                        .sin();
                        phase += 440.0 / f64::from(sample_rate)
                            * (wave * f64::from(amount) / 100.0).exp2();
                    }
                }
            }
        }
    }

    #[test]
    fn exact_copy_live_transfer_reset_and_invalid_event_clock_refusals() {
        let factory = factory(true, 4, true);
        eprintln!(
            "LFO runtime twin={} pair={}",
            factory.declared_twin_bytes(),
            2 * factory.declared_twin_bytes()
        );
        let mut source = factory.create_runtime();
        let on = [NativeInstrumentEvent::NoteOn {
            frame_offset: 7,
            occurrence_id: 4,
            pitch: 69,
            velocity: 100,
        }];
        source
            .preflight_events(&on, 91, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let mut scratch = [[0.0; 2]; 91];
        source.process_block_with_clock(
            &on,
            &mut scratch,
            Some(&Clock(0)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert!(
            source
                .preflight_clock(None, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_err()
        );
        struct Backwards;
        impl NativeInstrumentClock for Backwards {
            fn beat_at_offset(&self, offset: usize) -> Option<u64> {
                Some(10 - offset as u64)
            }
        }
        assert!(
            source
                .preflight_clock(Some(&Backwards), 2, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_err()
        );
        let capacity: Vec<_> = (10..27)
            .map(|id| NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: id,
                pitch: 60,
                velocity: 100,
            })
            .collect();
        assert_eq!(
            factory.create_runtime().preflight_events(
                &capacity,
                1,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            ),
            Err(Error::EventCapacityExceeded)
        );
        let at_end = [NativeInstrumentEvent::NoteOff {
            frame_offset: 91,
            occurrence_id: 4,
        }];
        assert!(
            source
                .preflight_events(&at_end, 91, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_err()
        );

        let duplicate = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 4,
            pitch: 69,
            velocity: 100,
        }];
        assert!(
            source
                .preflight_events(&duplicate, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_err()
        );
        let mut copied = factory.create_runtime();
        assert!(copied.can_copy_history_from(source.as_ref()));
        copied.copy_history_from(source.as_ref());
        let mut a = [[0.0; 2]; 113];
        let mut b = a;
        source.process_block_with_clock(
            &[],
            &mut a,
            Some(&Clock(91)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        copied.process_block_with_clock(
            &[],
            &mut b,
            Some(&Clock(91)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(a, b);
        let mut target = factory.clone();
        target.lfo.0[3] = 7.0_f32.to_bits();
        target.lfo.0[5] = 25.0_f32.to_bits();
        let InstrumentHistoryRetargetPreparation::Prepared(proof) =
            target.prepare_history_retarget_target().unwrap()
        else {
            panic!("target proof")
        };
        let mut edited = target.create_runtime();
        assert!(edited.can_retarget_history_from(source.as_ref(), proof.as_ref()));
        edited.retarget_history_from(source.as_ref(), proof.as_ref());
        let mut first = [[0.0; 2]; 1];
        let mut old = first;
        source.process_block_with_clock(
            &[],
            &mut old,
            Some(&Clock(204)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        edited.process_block_with_clock(
            &[],
            &mut first,
            Some(&Clock(204)),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(first, old, "edit preserves first oscillator phase");
        let mut different = target.clone();
        different.base[0] ^= 1;
        assert!(
            !different
                .create_runtime()
                .can_retarget_history_from(source.as_ref(), proof.as_ref())
        );
        edited.reset();
        assert!(edited.is_quiescent());
        let high = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 5,
            pitch: 127,
            velocity: 100,
        }];
        assert!(
            edited
                .preflight_events(&high, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_ok()
        );
    }
}
