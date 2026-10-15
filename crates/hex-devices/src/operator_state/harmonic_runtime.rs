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

mod spectrum;
pub(super) use spectrum::Spectrum;
use spectrum::{HarmonicPhaseTable, harmonic_phase_table};
const VOICES: usize = 64;

#[derive(Clone)]
pub(super) struct Factory {
    pub configuration: PreparedOperatorConfiguration,
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub settings: OperatorSettings,
    target: Spectrum,
    table: &'static SineTable,
    bank: &'static OperatorWaveformBank,
    phase_table: &'static HarmonicPhaseTable,
    rate: u32,
    parameters: [f64; 12],
}
impl Factory {
    pub fn new(
        settings: OperatorSettings,
        configuration: PreparedOperatorConfiguration,
        table: &'static SineTable,
        rate: u32,
    ) -> Result<Self, DeviceContractError> {
        let state = settings.to_state()?;
        let parameters = parameter_words(
            configuration
                .feedback_parameters(settings.effective_feedback().map(f64::from))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?,
        );
        Ok(Self {
            configuration,
            base: state.payload()[..ENABLE_PAYLOAD_BYTES]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
            target: Spectrum::prepare(&settings),
            settings,
            table,
            phase_table: harmonic_phase_table(),
            bank: prepared_bank().map_err(|_| DeviceContractError::RuntimeAllocation)?,
            rate,
            parameters,
        })
    }
    fn capacity(&self) -> usize {
        if self.settings.lfo.active() || self.settings.effective_feedback() != [0.0; 4] {
            16
        } else {
            64
        }
    }
    fn compatible(&self, other: &Self) -> bool {
        self.rate == other.rate && compatible_base(&self.base, &other.base)
    }
    fn parameters(&self) -> [f64; 12] {
        self.parameters
    }
}
fn compatible_base(a: &[u8; ENABLE_PAYLOAD_BYTES], b: &[u8; ENABLE_PAYLOAD_BYTES]) -> bool {
    a.iter().zip(b).enumerate().all(|(i, (a, b))| {
        let levels = [4..8, 16..20, 28..32, 40..44]
            .iter()
            .any(|r| r.contains(&i));
        let carriers = (112..128).contains(&i);
        let waveforms = (LEGACY_PAYLOAD_BYTES..WAVEFORM_PAYLOAD_BYTES).contains(&i);
        let feedback = (VELOCITY_PAYLOAD_BYTES..FEEDBACK_PAYLOAD_BYTES).contains(&i);
        let enabled = (LFO_PAYLOAD_BYTES..ENABLE_PAYLOAD_BYTES).contains(&i);
        let lfo = (FEEDBACK_PAYLOAD_BYTES..LFO_PAYLOAD_BYTES).contains(&i);
        levels || carriers || waveforms || feedback || enabled || lfo || a == b
    })
}
#[derive(Clone, Copy, Default)]
pub(super) struct Voice {
    pub note: PreparedOperatorNote,
    pub phases: [u64; 4],
    pub previous: [f64; 4],
    pub age: u64,
    pub gate: Option<u64>,
    pub id: u64,
    pub lfo_fraction: f64,
    pub lfo_cycle: u64,
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
#[derive(Clone)]
pub(super) struct Runtime {
    factory: Factory,
    voices: [Voice; VOICES],
    source: Spectrum,
    elapsed: u32,
    frames: u32,
    parameters: Option<ParameterRamp>,
    phase: (f64, u64),
}
#[derive(Clone, Copy)]
struct Target(u64);
impl PreparedInstrumentHistoryRetarget for Target {
    fn supports_source_runtime_version(&self, version: u32) -> bool {
        matches!(version, 1..=3)
    }
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        3
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        self.capacity()
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.configuration.release_frames()
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            factory: self.clone(),
            voices: [Voice::default(); VOICES],
            source: self.target,
            elapsed: 0,
            frames: 1,
            parameters: None,
            phase: (0.0, 0),
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
            if voice.id != 0
                && voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(release))
            {
                *voice = Voice::default();
            }
        }
    }
    fn advance(&mut self, n: usize) -> Result<(), Error> {
        for voice in &mut self.voices {
            if voice.id != 0 {
                voice.age = voice
                    .age
                    .checked_add(n as u64)
                    .ok_or(Error::InvalidEventBatch)?;
            }
        }
        self.expire();
        Ok(())
    }
    fn apply(&mut self, event: NativeInstrumentEvent) -> Result<(), Error> {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0 || self.voices.iter().any(|v| v.id == occurrence_id) {
                    return Err(Error::InvalidEventBatch);
                }
                let note = if self.factory.settings.lfo.active() {
                    self.factory.configuration.prepare_lfo_note(pitch, velocity)
                } else {
                    self.factory.configuration.prepare_note(pitch, velocity)
                }
                .map_err(|_| Error::InvalidEventBatch)?;
                let capacity = self.factory.capacity();
                let voice = self.voices[..capacity]
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
                *voice = Voice {
                    note,
                    id: occurrence_id,
                    ..Voice::default()
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
                self.expire();
            }
        }
        Ok(())
    }
}
impl MutableInstrumentRuntime for Runtime {
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
        if !compatible_base(&self.factory.base, base)
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
                && voice.gate_frames.is_none_or(|gate| {
                    gate <= voice.age_frames
                        && gate
                            .checked_add(voice.note.release_frames())
                            .is_some_and(|end| end > voice.age_frames)
                })
                && source.continuation_note(&voice.note).is_ok_and(|note| {
                    !self.factory.settings.lfo.active()
                        || PreparedOperatorConfiguration::lfo_steps(&note)
                            .iter()
                            .all(|step| *step < (1u64 << 62))
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
        self.voices = [Voice::default(); VOICES];
        for (target, voice) in self.voices.iter_mut().zip(history.voices) {
            let Ok(note) = source.continuation_note(&voice.note) else {
                return;
            };
            *target = Voice {
                note,
                phases: PreparedOperatorConfiguration::harmonic_initial_phases(
                    &note,
                    voice.age_frames,
                ),
                age: voice.age_frames,
                gate: voice.gate_frames,
                id: voice.occurrence_id,
                ..Voice::default()
            };
        }
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
    ) -> Result<(), Error> {
        if events.len() > 256 {
            return Err(Error::EventCapacityExceeded);
        }
        let mut proof = self.clone();
        let mut offset = 0;
        for event in events {
            let at = match *event {
                NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
            };
            if at < offset || at >= frames {
                return Err(Error::InvalidEventBatch);
            }
            proof.advance(at - offset)?;
            proof.apply(*event)?;
            offset = at;
        }
        proof.advance(frames - offset)
    }
    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
    ) -> Result<(), Error> {
        if !self.factory.settings.lfo.active() || !self.factory.settings.lfo.sync() {
            return Ok(());
        }
        let c = clock.ok_or(Error::InvalidEventBatch)?;
        let mut before = c.beat_at_offset(0).ok_or(Error::InvalidEventBatch)?;
        for offset in 1..=frames {
            let next = c.beat_at_offset(offset).ok_or(Error::InvalidEventBatch)?;
            if next < before {
                return Err(Error::InvalidEventBatch);
            }
            before = next;
        }
        Ok(())
    }
    fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
        self.process_block_with_clock(events, frames, None);
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let mut cursor = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.get(cursor) {
                let at = match *event {
                    NativeInstrumentEvent::NoteOn { frame_offset, .. }
                    | NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
                };
                if at != offset {
                    break;
                }
                let _ = self.apply(*event);
                cursor += 1;
            }
            let spectrum = self.spectrum();
            let parameters = self.current_parameters();
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
            for voice in &mut self.voices {
                if voice.id == 0 {
                    continue;
                }
                let phase = if law.retrigger() {
                    (voice.lfo_fraction, voice.lfo_cycle)
                } else {
                    self.phase
                };
                let multiplier = if law.active() {
                    (lfo::waveform(law.0[1], phase.0, phase.1) * law.amount()).exp2()
                } else {
                    1.0
                };
                let (sample, previous) = self.factory.configuration.sample_with_waveform_sampler(
                    &voice.note,
                    voice.age,
                    voice.gate.unwrap_or(u64::MAX),
                    &voice.phases,
                    voice.previous,
                    std::array::from_fn(|i| parameters[i]),
                    std::array::from_fn(|i| parameters[4 + i]),
                    std::array::from_fn(|i| parameters[8 + i]),
                    |operator, phase, step| {
                        spectrum.sample(
                            operator,
                            phase,
                            if law.active() {
                                step.saturating_mul(2)
                            } else {
                                step
                            },
                            self.factory.table,
                            self.factory.bank,
                            self.factory.phase_table,
                        )
                    },
                );
                frame[0] += sample;
                frame[1] += sample;
                voice.previous = previous;
                for (i, step) in PreparedOperatorConfiguration::lfo_steps(&voice.note)
                    .iter()
                    .enumerate()
                {
                    let step = if law.active() && law.selected(i) {
                        (*step as f64 * multiplier).round() as u64
                    } else {
                        *step
                    };
                    voice.phases[i] = voice.phases[i].wrapping_add(step);
                }
                let next = voice.lfo_fraction + delta;
                voice.lfo_cycle = voice.lfo_cycle.wrapping_add(next.floor() as u64);
                voice.lfo_fraction = next.fract();
                voice.age = voice.age.saturating_add(1);
            }
            let next = self.phase.0 + delta;
            self.phase.1 = self.phase.1.wrapping_add(next.floor() as u64);
            self.phase.0 = next.fract();
            self.elapsed = self.elapsed.saturating_add(1).min(self.frames);
            if let Some(r) = &mut self.parameters {
                r.elapsed += 1;
                if r.elapsed >= r.frames {
                    self.parameters = None;
                }
            }
            self.expire();
        }
    }
    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.source = self.factory.target;
        self.elapsed = 1;
        self.frames = 1;
        self.parameters = None;
        self.phase = (0.0, 0);
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|p| {
            self.factory.compatible(&p.factory) && self.factory.settings == p.factory.settings
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(p) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(p)
        {
            self.voices = p.voices;
            self.source = p.source;
            self.elapsed = p.elapsed;
            self.frames = p.frames;
            self.parameters = p.parameters;
            self.phase = p.phase;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        proof
            .as_any()
            .downcast_ref::<Target>()
            .is_some_and(|t| t.0 == self.factory.configuration.configuration_id())
            && Legacy::read(previous).is_some_and(|p| {
                compatible_base(&self.factory.base, &p.base)
                    && p.voices.iter().filter(|v| v.id != 0).count() <= self.factory.capacity()
                    && (!self.factory.settings.lfo.active()
                        || p.voices.iter().filter(|v| v.id != 0).all(|v| {
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
            self.voices = [Voice::default(); VOICES];
            for (target, source) in self
                .voices
                .iter_mut()
                .zip(p.voices.iter().filter(|voice| voice.id != 0))
            {
                *target = *source;
            }
            self.phase = p.phase;
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

pub(super) struct Legacy {
    pub base: [u8; ENABLE_PAYLOAD_BYTES],
    pub voices: [Voice; 64],
    pub parameters: [f64; 12],
    pub ramp: Option<ParameterRamp>,
    pub phase: (f64, u64),
    pub source: Spectrum,
    pub target: Spectrum,
    pub elapsed: u32,
    pub frames: u32,
}
pub(super) fn builtin_spectrum(base: &[u8; ENABLE_PAYLOAD_BYTES]) -> Option<Spectrum> {
    let mut out = Spectrum {
        coefficients: [[0.0; 64]; 4],
        builtin: [[0.0; 4]; 4],
    };
    for operator in 0..4 {
        let offset = LEGACY_PAYLOAD_BYTES + operator * 4;
        let shape = u32::from_le_bytes(base[offset..offset + 4].try_into().ok()?);
        let weight = out.builtin[operator].get_mut(shape as usize)?;
        *weight = 1.0;
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
impl Legacy {
    fn read(previous: &dyn MutableInstrumentRuntime) -> Option<Self> {
        if let Some(p) = previous.as_any().downcast_ref::<Runtime>() {
            return Some(Self {
                base: p.factory.base,
                voices: p.voices,
                parameters: p.factory.parameters(),
                ramp: p.parameters,
                phase: p.phase,
                source: p.source,
                target: p.factory.target,
                elapsed: p.elapsed,
                frames: p.frames,
            });
        }
        super::lfo_runtime::export_harmonic_history(previous)
            .or_else(|| super::feedback_runtime::export_harmonic_history(previous))
    }
}

#[cfg(test)]
mod tests;
