//! Bounded original pitch LFO runtime; no callback-owned heap buffers.
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
    feedback: [f64; 4],
    release: u64,
    rate: u32,
}

pub(super) fn prepare_factory(
    settings: &OperatorSettings,
    configuration: PreparedOperatorConfiguration,
    rate: u32,
    table: &'static SineTable,
) -> Result<Option<OperatorLfoFactory>, DeviceContractError> {
    if !settings.lfo.active() {
        return Ok(None);
    }
    let state = settings.to_state()?;
    Ok(Some(OperatorLfoFactory {
        release: configuration.release_frames(),
        configuration,
        table,
        base: state.payload()[..FEEDBACK_PAYLOAD_BYTES]
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?,
        lfo: settings.lfo,
        feedback: settings.effective_feedback().map(f64::from),
        rate,
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

#[derive(Clone)]
struct Runtime {
    factory: OperatorLfoFactory,
    voices: [Voice; VOICES],
    phase: Phase,
}

#[derive(Clone, Copy)]
struct Retarget(u64);
impl PreparedInstrumentHistoryRetarget for Retarget {
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedMutableInstrumentRuntime for OperatorLfoFactory {
    fn runtime_version(&self) -> u32 {
        2
    }
    fn declared_twin_bytes(&self) -> usize {
        size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        VOICES
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            factory: self.clone(),
            voices: [Voice::default(); VOICES],
            phase: Phase::default(),
        })
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

impl Runtime {
    fn compatible_base(&self, other: &Self) -> bool {
        self.factory.base == other.factory.base
            && self.factory.rate == other.factory.rate
            && std::ptr::eq(self.factory.table, other.factory.table)
    }
    fn copy(&mut self, other: &Self) {
        self.voices = other.voices;
        self.phase = other.phase;
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
                    .configuration
                    .prepare_lfo_note(pitch, velocity)
                    .map_err(|_| Error::InvalidEventBatch)?;
                let slot = self
                    .voices
                    .iter_mut()
                    .find(|v| v.id == 0)
                    .ok_or(Error::EventCapacityExceeded)?;
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

impl MutableInstrumentRuntime for Runtime {
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
            let next = match *event {
                NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
            };
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
    ) -> Result<(), Error> {
        if !self.factory.lfo.sync() {
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
    fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
        self.process_block_with_clock(events, frames, None);
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let law = self.factory.lfo;
        let mut event = 0;
        for (offset, frame) in frames.iter_mut().enumerate() {
            while let Some(next) = events.get(event) {
                let at = match *next {
                    NativeInstrumentEvent::NoteOn { frame_offset, .. }
                    | NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
                };
                if at != offset {
                    break;
                }
                let _ = self.apply(*next);
                event += 1;
            }
            let delta = if law.sync() {
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
            for voice in &mut self.voices {
                if voice.id == 0 {
                    continue;
                }
                let lfo = if law.retrigger() {
                    voice.lfo
                } else {
                    self.phase
                };
                let multiplier = (lfo.value(law.0[1]) * law.amount()).exp2();
                let (sample, previous) = self.factory.configuration.sample_lfo(
                    self.factory.table,
                    &voice.note,
                    voice.age,
                    voice.gate,
                    &voice.phases,
                    self.factory.feedback,
                    voice.previous,
                );
                frame[0] += sample;
                frame[1] += sample;
                voice.previous = previous;
                let steps = PreparedOperatorConfiguration::lfo_steps(&voice.note);
                for (i, step) in steps.iter().enumerate() {
                    let step = if law.selected(i) {
                        (*step as f64 * multiplier).round() as u64
                    } else {
                        *step
                    };
                    voice.phases[i] = voice.phases[i].wrapping_add(step);
                }
                voice.lfo.advance(delta);
                voice.age = voice.age.saturating_add(1);
            }
            self.phase.advance(delta);
            self.expire();
        }
    }
    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.phase = Phase::default();
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| self.compatible_base(p) && self.factory.lfo == p.factory.lfo)
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
        match (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<Retarget>(),
        ) {
            (Some(p), Some(proof)) => {
                proof.0 == self.factory.configuration.configuration_id() && self.compatible_base(p)
            }
            _ => false,
        }
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let Some(p) = previous.as_any().downcast_ref::<Self>()
            && self.can_retarget_history_from(p, proof)
        {
            self.copy(p);
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
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
            (37, ParameterValue::Float(0.0)),
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
            factory.declared_twin_bytes() * 2 <= 8192,
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
            runtime.preflight_events(&events, end - start).unwrap();
            runtime.preflight_clock(Some(&clock), end - start).unwrap();
            runtime.process_block_with_clock(&events, &mut output[start..end], Some(&clock));
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
                        super::super::OPERATOR_STATE_VERSION,
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
                    runtime.preflight_events(&events, frames.len()).unwrap();
                    runtime.process_block(&events, &mut frames);
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
        source.preflight_events(&on, 91).unwrap();
        let mut scratch = [[0.0; 2]; 91];
        source.process_block_with_clock(&on, &mut scratch, Some(&Clock(0)));
        assert!(source.preflight_clock(None, 1).is_err());
        struct Backwards;
        impl NativeInstrumentClock for Backwards {
            fn beat_at_offset(&self, offset: usize) -> Option<u64> {
                Some(10 - offset as u64)
            }
        }
        assert!(source.preflight_clock(Some(&Backwards), 2).is_err());
        let capacity: Vec<_> = (10..27)
            .map(|id| NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: id,
                pitch: 60,
                velocity: 100,
            })
            .collect();
        assert_eq!(
            factory.create_runtime().preflight_events(&capacity, 1),
            Err(Error::EventCapacityExceeded)
        );
        let at_end = [NativeInstrumentEvent::NoteOff {
            frame_offset: 91,
            occurrence_id: 4,
        }];
        assert!(source.preflight_events(&at_end, 91).is_err());

        let duplicate = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 4,
            pitch: 69,
            velocity: 100,
        }];
        assert!(source.preflight_events(&duplicate, 1).is_err());
        let mut copied = factory.create_runtime();
        assert!(copied.can_copy_history_from(source.as_ref()));
        copied.copy_history_from(source.as_ref());
        let mut a = [[0.0; 2]; 113];
        let mut b = a;
        source.process_block_with_clock(&[], &mut a, Some(&Clock(91)));
        copied.process_block_with_clock(&[], &mut b, Some(&Clock(91)));
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
        source.process_block_with_clock(&[], &mut old, Some(&Clock(204)));
        edited.process_block_with_clock(&[], &mut first, Some(&Clock(204)));
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
        assert!(edited.preflight_events(&high, 1).is_err());
    }
}
