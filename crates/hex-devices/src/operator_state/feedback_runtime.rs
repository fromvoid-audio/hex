//! Bounded native mutable Operator runtime for effective self-feedback.
use super::{FEEDBACK_COUNT, OperatorSettings, PAYLOAD_BYTES};
use crate::{SineTable, fm4::OPERATORS, fm4::PreparedOperatorConfiguration};
use hex_lattice::{
    InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime, NativeInstrumentEvent,
    NativeInstrumentRuntimeError, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime, StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::{any::Any, mem::size_of};

const MAX_VOICES: usize = 16;
const MAX_EVENTS: usize = 256;
const RUNTIME_VERSION: u32 = 1;

#[derive(Clone)]
pub(super) struct OperatorFeedbackFactory {
    configuration: PreparedOperatorConfiguration,
    sine_table: &'static SineTable,
    feedback_turns: [f64; OPERATORS],
    compatibility_state: [u8; PAYLOAD_BYTES],
    sample_rate: u32,
    release_frames: u64,
}

pub(super) fn prepare_factory(
    settings: &OperatorSettings,
    configuration: PreparedOperatorConfiguration,
    sample_rate: u32,
    sine_table: &'static SineTable,
) -> Result<Option<OperatorFeedbackFactory>, DeviceContractError> {
    let effective = settings.effective_feedback();
    if effective == [0.0; FEEDBACK_COUNT] {
        return Ok(None);
    }
    let feedback_turns = effective.map(f64::from);
    let native_state = settings.to_state()?;
    let compatibility_state = native_state
        .payload()
        .try_into()
        .map_err(|_| DeviceContractError::MalformedState)?;
    Ok(Some(OperatorFeedbackFactory {
        release_frames: configuration.release_frames(),
        configuration,
        sine_table,
        feedback_turns,
        compatibility_state,
        sample_rate,
    }))
}

impl PreparedMutableInstrumentRuntime for OperatorFeedbackFactory {
    fn runtime_version(&self) -> u32 {
        RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        size_of::<OperatorFeedbackRuntime>()
    }

    fn max_events_per_block(&self) -> usize {
        MAX_EVENTS
    }

    fn max_active_occurrences(&self) -> usize {
        MAX_VOICES
    }

    fn max_release_tail_frames(&self) -> u64 {
        self.release_frames
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(OperatorFeedbackRuntime {
            configuration: self.configuration.clone(),
            sine_table: self.sine_table,
            feedback_turns: self.feedback_turns,
            compatibility_state: self.compatibility_state,
            sample_rate: self.sample_rate,
            release_frames: self.release_frames,
            voices: [VoiceSlot::EMPTY; MAX_VOICES],
            retarget: None,
            retarget_elapsed: 0,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        let frames = crate::fm4::PreparedOperatorFeedbackRetarget::default_frames(self.sample_rate)
            .map_err(|_| DeviceContractError::MalformedState)?;
        let target = self
            .configuration
            .feedback_parameters(self.feedback_turns)
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            OperatorFeedbackHistoryRetarget {
                target_configuration_id: self.configuration.configuration_id(),
                target,
                frames,
            },
        )))
    }
}

#[derive(Clone, Copy)]
struct OperatorFeedbackHistoryRetarget {
    target_configuration_id: u64,
    target: crate::fm4::OperatorFeedbackParameters,
    frames: u32,
}

impl PreparedInstrumentHistoryRetarget for OperatorFeedbackHistoryRetarget {
    fn declared_fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct VoiceSlot {
    occurrence_id: u64,
    voice: Option<crate::fm4::PreparedOperatorFeedbackVoice>,
}

impl VoiceSlot {
    const EMPTY: Self = Self {
        occurrence_id: 0,
        voice: None,
    };
}

struct OperatorFeedbackRuntime {
    configuration: PreparedOperatorConfiguration,
    sine_table: &'static SineTable,
    feedback_turns: [f64; OPERATORS],
    compatibility_state: [u8; PAYLOAD_BYTES],
    sample_rate: u32,
    release_frames: u64,
    voices: [VoiceSlot; MAX_VOICES],
    retarget: Option<crate::fm4::PreparedOperatorFeedbackRetarget>,
    retarget_elapsed: u32,
}

#[derive(Clone, Copy, Default)]
struct VoiceProof {
    occurrence_id: u64,
    age_frames: u64,
    gate_frame: Option<u64>,
    active: bool,
}

impl OperatorFeedbackRuntime {
    fn proof(&self) -> [VoiceProof; MAX_VOICES] {
        std::array::from_fn(|index| {
            let slot = self.voices[index];
            if let Some(voice) = slot.voice {
                VoiceProof {
                    occurrence_id: slot.occurrence_id,
                    age_frames: voice.age_frames(),
                    gate_frame: voice.gate_frame(),
                    active: true,
                }
            } else {
                VoiceProof::default()
            }
        })
    }

    fn advance_proof(
        proof: &mut [VoiceProof; MAX_VOICES],
        frames: usize,
        release_frames: u64,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let frames =
            u64::try_from(frames).map_err(|_| NativeInstrumentRuntimeError::InvalidEventBatch)?;
        for voice in proof.iter_mut().filter(|voice| voice.active) {
            voice.age_frames = voice
                .age_frames
                .checked_add(frames)
                .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
            if let Some(gate) = voice.gate_frame {
                let end = gate
                    .checked_add(release_frames)
                    .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
                if voice.age_frames >= end {
                    *voice = VoiceProof::default();
                }
            }
        }
        Ok(())
    }

    fn expire_finished(&mut self) {
        for slot in &mut self.voices {
            let expired = slot.voice.is_some_and(|voice| {
                voice.gate_frame().is_some_and(|gate| {
                    gate.checked_add(self.release_frames)
                        .is_some_and(|end| voice.age_frames() >= end)
                })
            });
            if expired {
                *slot = VoiceSlot::EMPTY;
            }
        }
    }

    fn apply_event(&mut self, event: NativeInstrumentEvent) {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if let Some(slot) = self.voices.iter_mut().find(|slot| slot.voice.is_none())
                    && let Ok(voice) = self.configuration.prepare_feedback_voice(
                        pitch,
                        velocity,
                        self.feedback_turns,
                    )
                {
                    let mut voice = voice;
                    if let Some(ramp) = self.retarget
                        && voice
                            .enroll_in_retarget(&self.configuration, &ramp, self.retarget_elapsed)
                            .is_err()
                    {
                        return;
                    }
                    *slot = VoiceSlot {
                        occurrence_id,
                        voice: Some(voice),
                    };
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(slot) = self
                    .voices
                    .iter_mut()
                    .find(|slot| slot.occurrence_id == occurrence_id && slot.voice.is_some())
                    && let Some(voice) = slot.voice.as_mut()
                {
                    let _ = voice.note_off();
                    if self.release_frames == 0 {
                        *slot = VoiceSlot::EMPTY;
                    }
                }
            }
        }
    }

    fn process_voice_sample(&mut self, frame: &mut StereoFrame) {
        for slot in &mut self.voices {
            if let Some(voice) = slot.voice.as_mut() {
                let sample = if let Some(ramp) = self.retarget {
                    if self.retarget_elapsed < ramp.frames() {
                        voice.process_frame_with_retarget(
                            &self.configuration,
                            self.sine_table,
                            &ramp,
                            self.retarget_elapsed,
                        )
                    } else {
                        voice.process_frame(&self.configuration, self.sine_table)
                    }
                } else {
                    voice.process_frame(&self.configuration, self.sine_table)
                };
                if let Ok(sample) = sample {
                    frame[0] += sample;
                    frame[1] += sample;
                }
            }
        }
        if let Some(ramp) = self.retarget {
            self.retarget_elapsed = self.retarget_elapsed.saturating_add(1);
            if self.retarget_elapsed >= ramp.frames() {
                self.retarget = None;
                self.retarget_elapsed = 0;
            }
        }
        self.expire_finished();
    }

    fn proposed_retarget(
        &self,
        previous: &Self,
        proof: &OperatorFeedbackHistoryRetarget,
    ) -> Option<crate::fm4::PreparedOperatorFeedbackRetarget> {
        if self.configuration.configuration_id() != proof.target_configuration_id
            || self
                .configuration
                .feedback_parameters(self.feedback_turns)
                .ok()?
                != proof.target
            || !previous
                .configuration
                .retarget_compatible_with(&self.configuration)
        {
            return None;
        }
        if let Some(ramp) = previous.retarget {
            ramp.continue_from_shared_ramp(
                &previous.configuration,
                &self.configuration,
                self.feedback_turns,
                previous.retarget_elapsed,
                proof.frames,
            )
            .ok()
        } else {
            let start = previous
                .configuration
                .feedback_parameters(previous.feedback_turns)
                .ok()?;
            crate::fm4::PreparedOperatorFeedbackRetarget::prepare_from_shared_parameters(
                &previous.configuration,
                &self.configuration,
                start,
                self.feedback_turns,
                proof.frames,
            )
            .ok()
        }
    }

    fn can_retarget_from(&self, previous: &Self, proof: &OperatorFeedbackHistoryRetarget) -> bool {
        if previous.sample_rate != self.sample_rate
            || previous.release_frames != self.release_frames
            || !std::ptr::eq(previous.sine_table, self.sine_table)
        {
            return false;
        }
        let Some(ramp) = self.proposed_retarget(previous, proof) else {
            return false;
        };
        previous.voices.iter().all(|slot| {
            let Some(mut voice) = slot.voice else {
                return true;
            };
            voice
                .process_frame_with_retarget(&self.configuration, self.sine_table, &ramp, 0)
                .is_ok()
        })
    }

    fn apply_retarget_from(&mut self, previous: &Self, proof: &OperatorFeedbackHistoryRetarget) {
        let Some(ramp) = self.proposed_retarget(previous, proof) else {
            return;
        };
        self.voices = previous.voices;
        self.retarget = Some(ramp);
        self.retarget_elapsed = 0;
    }

    fn can_copy_history_from_runtime(&self, previous: &Self) -> bool {
        let rebound = match previous.retarget {
            Some(ramp) => match ramp.rebind_target_after_exact_history_copy(
                &previous.configuration,
                &self.configuration,
                previous.retarget_elapsed,
            ) {
                Ok(rebound) => Some(rebound),
                Err(_) => return false,
            },
            None => None,
        };
        previous.voices.iter().all(|slot| {
            slot.voice.is_none_or(|voice| {
                voice.can_rebind_after_exact_history_copy(
                    &previous.configuration,
                    &self.configuration,
                    previous
                        .retarget
                        .as_ref()
                        .zip(rebound.as_ref())
                        .map(|(original, rebound)| (original, rebound, previous.retarget_elapsed)),
                )
            })
        })
    }
}

impl MutableInstrumentRuntime for OperatorFeedbackRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > MAX_EVENTS {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut proof = self.proof();
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let offset = match event {
                NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
            };
            if offset >= frame_count || offset < previous_offset {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            Self::advance_proof(&mut proof, offset - previous_offset, self.release_frames)?;
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    velocity,
                    ..
                } => {
                    if occurrence_id == 0
                        || proof
                            .iter()
                            .any(|voice| voice.active && voice.occurrence_id == occurrence_id)
                        || self
                            .configuration
                            .prepare_feedback_voice(pitch, velocity, self.feedback_turns)
                            .is_err()
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    let elapsed_at_event = if self.retarget.is_some() {
                        self.retarget_elapsed
                            .checked_add(
                                u32::try_from(offset)
                                    .map_err(|_| NativeInstrumentRuntimeError::InvalidEventBatch)?,
                            )
                            .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?
                    } else {
                        0
                    };
                    if let Some(ramp) = self.retarget
                        && elapsed_at_event < ramp.frames()
                    {
                        self.configuration
                            .prepare_feedback_voice(pitch, velocity, self.feedback_turns)
                            .and_then(|mut voice| {
                                voice.enroll_in_retarget(
                                    &self.configuration,
                                    &ramp,
                                    elapsed_at_event,
                                )?;
                                Ok(voice)
                            })
                            .map_err(|_| NativeInstrumentRuntimeError::InvalidEventBatch)?;
                    }
                    let slot = proof
                        .iter_mut()
                        .find(|voice| !voice.active)
                        .ok_or(NativeInstrumentRuntimeError::EventCapacityExceeded)?;
                    *slot = VoiceProof {
                        occurrence_id,
                        age_frames: 0,
                        gate_frame: None,
                        active: true,
                    };
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let voice = proof
                        .iter_mut()
                        .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                        .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
                    if voice.gate_frame.is_some() {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    voice
                        .age_frames
                        .checked_add(self.release_frames)
                        .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
                    voice.gate_frame = Some(voice.age_frames);
                    if self.release_frames == 0 {
                        *voice = VoiceProof::default();
                    }
                }
            }
            previous_offset = offset;
        }
        let remaining = frame_count
            .checked_sub(previous_offset)
            .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
        Self::advance_proof(&mut proof, remaining, self.release_frames)?;
        Ok(())
    }

    fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
        // The lattice pair calls this only after generic and device preflight.
        // The exact immutable configuration and events were validated there;
        // admitted note construction and sampling are allocation-free.
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while events.get(event_index).is_some_and(|event| match *event {
                NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | NativeInstrumentEvent::NoteOff { frame_offset, .. } => {
                    frame_offset == frame_index
                }
            }) {
                self.apply_event(events[event_index]);
                event_index += 1;
            }
            self.process_voice_sample(frame);
        }
    }

    fn reset(&mut self) {
        self.voices = [VoiceSlot::EMPTY; MAX_VOICES];
        self.retarget = None;
        self.retarget_elapsed = 0;
    }

    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|slot| slot.voice.is_none())
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.compatibility_state == previous.compatibility_state
                    && self.sample_rate == previous.sample_rate
                    && self.feedback_turns == previous.feedback_turns
                    && self.release_frames == previous.release_frames
                    && std::ptr::eq(self.sine_table, previous.sine_table)
                    && self.can_copy_history_from_runtime(previous)
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(previous)
        {
            let rebound = match previous.retarget {
                Some(ramp) => match ramp.rebind_target_after_exact_history_copy(
                    &previous.configuration,
                    &self.configuration,
                    previous.retarget_elapsed,
                ) {
                    Ok(rebound) => Some(rebound),
                    Err(_) => return,
                },
                None => None,
            };
            self.voices = previous.voices;
            if let (Some(original), Some(rebound)) = (previous.retarget, rebound) {
                for slot in &mut self.voices {
                    if let Some(voice) = slot.voice.as_mut() {
                        let _ = voice.rebind_after_exact_history_copy_with_ramp(
                            &previous.configuration,
                            &self.configuration,
                            Some((&original, &rebound, previous.retarget_elapsed)),
                        );
                    }
                }
                self.retarget = Some(rebound);
                self.retarget_elapsed = previous.retarget_elapsed;
            } else {
                for slot in &mut self.voices {
                    if let Some(voice) = slot.voice.as_mut() {
                        let _ = voice.rebind_after_exact_history_copy_with_ramp(
                            &previous.configuration,
                            &self.configuration,
                            None,
                        );
                    }
                }
                self.retarget = None;
                self.retarget_elapsed = 0;
            }
        }
    }

    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof
                .as_any()
                .downcast_ref::<OperatorFeedbackHistoryRetarget>(),
        ) else {
            return false;
        };
        self.can_retarget_from(previous, proof)
    }

    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof
                .as_any()
                .downcast_ref::<OperatorFeedbackHistoryRetarget>(),
        ) && self.can_retarget_from(previous, proof)
        {
            self.apply_retarget_from(previous, proof);
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        size_of::<Self>()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_state::{OPERATOR_METADATA, parameter};
    use crate::{fm4::PreparedOperatorFeedbackVoice, operator_state::OperatorSettings};
    use hex_lattice::{NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    const RATE: u32 = 48_000;

    fn factory(amount: f32) -> OperatorFeedbackFactory {
        factory_with_parameters(amount, 1.0, 1.0)
    }

    fn factory_with_parameters(feedback: f32, level: f32, carrier: f32) -> OperatorFeedbackFactory {
        let state = OPERATOR_METADATA
            .patch_parameter(
                &OPERATOR_METADATA.default_state().expect("defaults"),
                parameter::oscillator_level(1),
                ParameterValue::Float(level),
            )
            .expect("oscillator level state");
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::carrier(1),
                ParameterValue::Float(carrier),
            )
            .expect("carrier state");
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::feedback(1),
                ParameterValue::Float(feedback),
            )
            .expect("feedback state");
        let settings = OperatorSettings::from_state(&state).expect("settings");
        let configuration = settings.prepare_configuration(RATE).expect("configuration");
        prepare_factory(&settings, configuration, RATE, crate::shared_sine_table())
            .expect("factory preparation")
            .expect("nonzero feedback factory")
    }

    fn render_partitioned(partition: usize) -> Vec<StereoFrame> {
        let factory = factory(0.45);
        assert!(factory.declared_twin_bytes() <= 4096);
        let config = factory.configuration.clone();
        let mut standalone: PreparedOperatorFeedbackVoice = config
            .prepare_feedback_voice(60, 103, factory.feedback_turns)
            .expect("standalone feedback voice");
        let mut runtime = factory.create_runtime();
        let mut output = vec![[0.0; 2]; 640];
        let mut expected = vec![[0.0; 2]; 640];
        let mut frame = 0;
        while frame < output.len() {
            let length = partition.min(output.len() - frame);
            let mut events = Vec::with_capacity(2);
            if frame == 0 {
                events.push(NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 103,
                });
            }
            if (frame..frame + length).contains(&200) {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 200 - frame,
                    occurrence_id: 1,
                });
            }
            let block = &mut output[frame..frame + length];
            runtime
                .preflight_events(&events, length)
                .expect("accepted bounded event block");
            runtime.process_block(&events, block);
            for (local, sample) in expected[frame..frame + length].iter_mut().enumerate() {
                let absolute = frame + local;
                if absolute == 200 {
                    standalone.note_off().expect("note off");
                }
                let value = standalone
                    .process_frame(&config, factory.sine_table)
                    .expect("standalone sample");
                *sample = [value, value];
            }
            frame += length;
        }
        assert_eq!(output, expected);
        output
    }

    #[test]
    fn registered_feedback_matches_standalone_across_partitions_and_release() {
        let expected = render_partitioned(1);
        for partition in [17, 64, 127, 128, 256] {
            assert_eq!(
                render_partitioned(partition),
                expected,
                "partition {partition}"
            );
        }
    }

    #[test]
    fn native_factory_declares_exact_storage_and_refuses_seventeenth_voice() {
        let factory = factory(0.4);
        let declared = factory.declared_twin_bytes();
        assert_eq!(declared, size_of::<OperatorFeedbackRuntime>());
        assert_eq!(declared, 4_032, "actual twin storage {declared}");
        assert!(
            2 * declared <= 8_192,
            "paired runtime storage {}",
            2 * declared
        );
        assert_eq!(2 * declared, 8_064, "actual paired runtime storage");
        let pair = NativeInstrumentRuntimePair::new(
            &factory,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 8_192,
                max_events_per_block: MAX_EVENTS,
                max_active_occurrences: MAX_VOICES,
                max_tail_frames: factory.release_frames,
            },
        )
        .expect("actual prepared pair remains within the fixed ceiling");
        assert_eq!(pair.fixed_storage_pair_bytes(), Some(8_064));
        let runtime = factory.create_runtime();
        let mut events = Vec::with_capacity(17);
        for occurrence_id in 1..=17 {
            events.push(NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id,
                pitch: 60,
                velocity: 100,
            });
        }
        assert_eq!(
            runtime.preflight_events(&events, 1),
            Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
        );
        let one = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 21,
            pitch: 60,
            velocity: 100,
        }];
        runtime
            .preflight_events(&one, 1)
            .expect("failed preflight leaves every slot available");
    }

    #[test]
    fn retarget_handles_held_released_new_notes_rapid_edits_and_exact_copy() {
        let old = factory_with_parameters(0.15, 0.4, 0.25);
        let middle = factory_with_parameters(0.35, 0.7, 0.5);
        let target = factory_with_parameters(0.6, 0.9, 0.8);
        let InstrumentHistoryRetargetPreparation::Prepared(proof) = middle
            .prepare_history_retarget_target()
            .expect("prepare supported edit")
        else {
            panic!("supported edit produces a retarget proof");
        };
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            proof.declared_fixed_storage_bytes(),
            112,
            "the target-bound control proof has a fixed, accounted layout"
        );
        let wrong_target = target.create_runtime();
        assert!(
            !wrong_target.can_retarget_history_from(old.create_runtime().as_ref(), proof.as_ref()),
            "a proof for the exact middle target cannot authorize another configuration"
        );
        let mut source = old.create_runtime();
        let initial_events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 103,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 67,
                velocity: 91,
            },
        ];
        let mut initial = [[0.0; 2]; 17];
        source
            .preflight_events(&initial_events, initial.len())
            .expect("initial notes fit");
        source.process_block(&initial_events, &mut initial);
        let note_off = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }];
        let mut release = [[0.0; 2]; 9];
        source
            .preflight_events(&note_off, release.len())
            .expect("note off accepted");
        source.process_block(&note_off, &mut release);

        let mut ramped = middle.create_runtime();
        assert!(ramped.can_retarget_history_from(source.as_ref(), proof.as_ref()));
        ramped.retarget_history_from(source.as_ref(), proof.as_ref());
        let mut first_ramp = [[0.0; 2]; 31];
        ramped
            .preflight_events(&[], first_ramp.len())
            .expect("empty events");
        ramped.process_block(&[], &mut first_ramp);

        let InstrumentHistoryRetargetPreparation::Prepared(second_proof) = target
            .prepare_history_retarget_target()
            .expect("prepare rapid supported edit")
        else {
            panic!("rapid supported edit produces a retarget proof");
        };
        let mut retargeted = target.create_runtime();
        assert!(retargeted.can_retarget_history_from(ramped.as_ref(), second_proof.as_ref()));
        retargeted.retarget_history_from(ramped.as_ref(), second_proof.as_ref());

        // Same-offset NoteOff follows NoteOn: state-only enrollment keeps its
        // fresh age at zero, so its release begins at this exact boundary.
        let same_offset = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 3,
                pitch: 72,
                velocity: 85,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 3,
            },
        ];
        let mut joined = [[0.0; 2]; 13];
        retargeted
            .preflight_events(&same_offset, joined.len())
            .expect("same-offset note joins shared ramp before note-off");
        retargeted.process_block(&same_offset, &mut joined);

        let mut exact_copy = target.create_runtime();
        assert!(exact_copy.can_copy_history_from(retargeted.as_ref()));
        exact_copy.copy_history_from(retargeted.as_ref());
        let mut continued = [[0.0; 2]; 97];
        let mut copied = continued;
        retargeted
            .preflight_events(&[], continued.len())
            .expect("continuation");
        exact_copy
            .preflight_events(&[], copied.len())
            .expect("copied continuation");
        retargeted.process_block(&[], &mut continued);
        exact_copy.process_block(&[], &mut copied);
        assert_eq!(
            continued, copied,
            "active-ramp exact copy must preserve future PCM"
        );

        let unsupported_state = OPERATOR_METADATA
            .patch_parameter(
                &OPERATOR_METADATA.default_state().expect("defaults"),
                parameter::waveform(1),
                ParameterValue::Choice(1),
            )
            .expect("waveform state");
        let unsupported_state = OPERATOR_METADATA
            .patch_parameter(
                &unsupported_state,
                parameter::feedback(1),
                ParameterValue::Float(0.5),
            )
            .expect("mutable feedback state");
        let unsupported_settings =
            OperatorSettings::from_state(&unsupported_state).expect("unsupported settings");
        let unsupported_configuration = unsupported_settings
            .prepare_configuration(RATE)
            .expect("supported waveform configuration");
        let unsupported = prepare_factory(
            &unsupported_settings,
            unsupported_configuration,
            RATE,
            crate::shared_sine_table(),
        )
        .expect("factory preparation")
        .expect("mutable feedback factory");
        let InstrumentHistoryRetargetPreparation::Prepared(proof) = target
            .prepare_history_retarget_target()
            .expect("target proof is source-independent")
        else {
            panic!("mutable Operator target has a retarget proof");
        };
        let unsupported_runtime = unsupported.create_runtime();
        let target_runtime = target.create_runtime();
        assert!(
            !target_runtime
                .can_retarget_history_from(unsupported_runtime.as_ref(), proof.as_ref(),)
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn release_end_overflow_refuses_without_consuming_runtime_capacity() {
        let runtime = factory(0.4).create_runtime();
        let near_limit = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 31,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: usize::MAX - 1,
                occurrence_id: 31,
            },
        ];
        assert_eq!(
            runtime.preflight_events(&near_limit, usize::MAX),
            Err(NativeInstrumentRuntimeError::InvalidEventBatch)
        );

        let retry = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 32,
            pitch: 60,
            velocity: 100,
        }];
        runtime
            .preflight_events(&retry, 1)
            .expect("overflow refusal leaves runtime slots unchanged");
    }

    #[test]
    fn history_copy_requires_exact_state_and_rate_compatibility() {
        let same_a = factory(0.4).create_runtime();
        let same_b = factory(0.4).create_runtime();
        let changed = factory(0.5).create_runtime();
        assert!(same_a.can_copy_history_from(same_b.as_ref()));
        assert!(!same_a.can_copy_history_from(changed.as_ref()));
    }

    #[test]
    fn zero_effective_feedback_keeps_analytic_factory_absent() {
        let state = OPERATOR_METADATA.default_state().expect("defaults");
        let settings = OperatorSettings::from_state(&state).expect("settings");
        assert!(
            prepare_factory(
                &settings,
                settings.prepare_configuration(RATE).expect("configuration"),
                RATE,
                crate::shared_sine_table(),
            )
            .expect("prepare")
            .is_none()
        );
    }
}
