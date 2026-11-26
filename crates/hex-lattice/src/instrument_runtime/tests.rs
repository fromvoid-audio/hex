mod active_occurrences;
use super::*;
use crate::{ANALYTIC_NOTE_PAYLOAD_WORDS, PreparedAnalyticNote, PreparedNativeInstrument};
use hex_lattice_contract::DeviceContractError;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const TEST_LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
    max_pair_bytes: 4_096,
    max_events_per_block: 8,
    max_active_occurrences: 2,
    max_tail_frames: 16,
};

#[derive(Clone, Copy, Default)]
struct Voice {
    occurrence_id: u64,
    pitch: u8,
    velocity: u8,
    phase: f32,
    release_left: u8,
    active: bool,
    gated: bool,
}

struct FeedbackPrepared {
    observation: Option<active_occurrences::Observation>,
    compatibility_key: u8,
    release_frames: u8,
    max_tail_frames: u64,
    max_events: usize,
    max_voices: usize,
    declared_bytes: usize,
    creations: Arc<AtomicUsize>,
    alternate_twin_key: bool,
}

struct FeedbackRuntime {
    observation: Option<active_occurrences::Observation>,
    compatibility_key: u8,
    release_frames: u8,
    max_tail_frames: u64,
    tail_frames_remaining: u64,
    max_voices: usize,
    voices: [Voice; 2],
    feedback: f32,
}

impl FeedbackRuntime {
    fn apply_event(&mut self, event: NativeInstrumentEvent) {
        match event {
            NativeInstrumentEvent::Expression { .. }
            | NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. }
            | NativeInstrumentEvent::NotePressure { .. }
            | NativeInstrumentEvent::NotePitchBend { .. }
            | NativeInstrumentEvent::NoteTimbre { .. } => {}
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if let Some(voice) = self
                    .voices
                    .get_mut(..self.max_voices)
                    .and_then(|voices| voices.iter_mut().find(|voice| !voice.active))
                {
                    *voice = Voice {
                        occurrence_id,
                        pitch,
                        velocity,
                        phase: 0.0,
                        release_left: self.release_frames,
                        active: true,
                        gated: true,
                    };
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = self
                    .voices
                    .iter_mut()
                    .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                {
                    voice.gated = false;
                    voice.release_left = self.release_frames;
                    if self.release_frames == 0 {
                        *voice = Voice::default();
                        if self.voices.iter().all(|voice| !voice.active) {
                            self.tail_frames_remaining = self.max_tail_frames;
                        }
                    }
                }
            }
        }
    }

    fn advance_release(voice: &mut Voice) {
        if voice.active && !voice.gated {
            voice.release_left = voice.release_left.saturating_sub(1);
            if voice.release_left == 0 {
                *voice = Voice::default();
            }
        }
    }

    fn advance_many(voices: &mut [Voice; 2], frames: usize) {
        for _ in 0..frames {
            for voice in voices.iter_mut() {
                Self::advance_release(voice);
            }
        }
    }
}

impl PreparedMutableInstrumentRuntime for FeedbackPrepared {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.observation.is_some()
    }
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_twin_bytes(&self) -> usize {
        self.declared_bytes
    }

    fn max_events_per_block(&self) -> usize {
        self.max_events
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_voices
    }

    fn max_release_tail_frames(&self) -> u64 {
        u64::from(self.release_frames).saturating_add(self.max_tail_frames)
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let ordinal = self.creations.fetch_add(1, Ordering::Relaxed);
        Box::new(FeedbackRuntime {
            observation: self.observation,
            compatibility_key: if self.alternate_twin_key {
                self.compatibility_key.wrapping_add(ordinal as u8)
            } else {
                self.compatibility_key
            },
            release_frames: self.release_frames,
            max_tail_frames: self.max_tail_frames,
            tail_frames_remaining: 0,
            max_voices: self.max_voices,
            voices: [Voice::default(); 2],
            feedback: 0.0,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for FeedbackRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.observation.is_some()
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        active_occurrences::observe(self, output)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let mut voices = self.voices;
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let offset = event.frame_offset();
            Self::advance_many(&mut voices, offset - previous_offset);
            match event {
                NativeInstrumentEvent::NotePressure { .. } => {
                    return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. }
                | NativeInstrumentEvent::NotePitchBend { .. }
                | NativeInstrumentEvent::NoteTimbre { .. } => {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    let Some(active_voices) = voices.get_mut(..self.max_voices) else {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    };
                    if active_voices
                        .iter()
                        .any(|voice| voice.active && voice.occurrence_id == occurrence_id)
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    let Some(slot) = active_voices.iter_mut().find(|voice| !voice.active) else {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    };
                    *slot = Voice {
                        occurrence_id,
                        active: true,
                        gated: true,
                        release_left: self.release_frames,
                        ..Voice::default()
                    };
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let Some(voice) = voices
                        .iter_mut()
                        .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                    else {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    };
                    if !voice.gated {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    voice.gated = false;
                    voice.release_left = self.release_frames;
                    if self.release_frames == 0 {
                        *voice = Voice::default();
                    }
                }
            }
            previous_offset = offset;
        }
        let _ = frame_count;
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while events
                .get(event_index)
                .is_some_and(|event| event.frame_offset() == frame_index)
            {
                self.apply_event(events[event_index]);
                event_index += 1;
            }

            let had_active_voice = self.voices.iter().any(|voice| voice.active);
            let mut excitation = 0.0_f32;
            for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                let level = if voice.gated {
                    f32::from(voice.velocity) / 127.0
                } else {
                    f32::from(voice.release_left) / f32::from(self.release_frames.max(1))
                };
                excitation += voice.phase * level;
                voice.phase += f32::from(voice.pitch + 1) * 0.000_1;
                Self::advance_release(voice);
            }
            let has_active_voice = self.voices.iter().any(|voice| voice.active);
            if had_active_voice && !has_active_voice {
                self.tail_frames_remaining = self.max_tail_frames;
            }
            self.feedback = self.feedback * 0.5 + excitation * 0.25;
            if !has_active_voice && self.tail_frames_remaining == 0 {
                self.feedback = 0.0;
            }
            frame[0] += self.feedback;
            frame[1] -= self.feedback * 0.5;
            if !has_active_voice && self.tail_frames_remaining > 0 {
                self.tail_frames_remaining -= 1;
                if self.tail_frames_remaining == 0 {
                    self.feedback = 0.0;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::default(); 2];
        self.feedback = 0.0;
        self.tail_frames_remaining = 0;
    }

    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| !voice.active)
            && self.tail_frames_remaining == 0
            && self.feedback == 0.0
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                previous.compatibility_key == self.compatibility_key
                    && previous.release_frames == self.release_frames
                    && previous.max_tail_frames == self.max_tail_frames
                    && previous.max_voices == self.max_voices
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>()
            && self.can_copy_history_from(previous)
        {
            self.voices = previous.voices;
            self.feedback = previous.feedback;
            self.tail_frames_remaining = previous.tail_frames_remaining;
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn prepared(key: u8, release_frames: u8, creations: Arc<AtomicUsize>) -> FeedbackPrepared {
    FeedbackPrepared {
        observation: None,
        compatibility_key: key,
        release_frames,
        max_tail_frames: 4,
        max_events: 4,
        max_voices: 2,
        declared_bytes: std::mem::size_of::<FeedbackRuntime>(),
        creations,
        alternate_twin_key: false,
    }
}

fn event_on(offset: usize, id: u64, pitch: u8, velocity: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: offset,
        occurrence_id: id,
        pitch,
        velocity,
    }
}

#[test]
fn controller_domain_order_and_unsupported_provider_refusal_are_atomic() {
    use NativeInstrumentExpression::{ChannelPressure, ModWheel, PitchBend};
    for expression in [
        ChannelPressure(0),
        ChannelPressure(127),
        ModWheel(127),
        PitchBend(-8192),
        PitchBend(0),
        PitchBend(8191),
    ] {
        let event = NativeInstrumentEvent::Expression {
            frame_offset: 3,
            expression,
        };
        assert_eq!(validate_event_batch(&[event], 4, 4, false), Ok(()));
    }
    for expression in [
        ChannelPressure(128),
        ModWheel(255),
        PitchBend(-8193),
        PitchBend(8192),
    ] {
        assert_eq!(
            validate_event_batch(
                &[NativeInstrumentEvent::Expression {
                    frame_offset: 0,
                    expression
                }],
                1,
                4,
                false
            ),
            Err(NativeInstrumentRuntimeError::InvalidEventBatch)
        );
    }
    assert_eq!(PitchBend(-8192).normalized(), -1.0);
    assert_eq!(PitchBend(0).normalized(), 0.0);
    assert_eq!(PitchBend(8191).normalized(), 1.0);
    let expression = NativeInstrumentEvent::Expression {
        frame_offset: 0,
        expression: ModWheel(100),
    };
    let late = NativeInstrumentEvent::Expression {
        frame_offset: 4,
        expression: ModWheel(100),
    };
    assert!(validate_event_batch(&[late], 4, 4, false).is_err());
    assert!(validate_event_batch(&[event_on(1, 1, 60, 127), expression], 4, 4, false).is_err());
    let factory = prepared(1, 2, Arc::new(AtomicUsize::new(0)));
    let mut pair = NativeInstrumentRuntimePair::new(&factory, TEST_LIMITS).unwrap();
    let mut frames = [[0.123, -0.456]; 4];
    let unchanged = frames;
    pair.begin_block();
    assert_eq!(
        pair.process_block(&[expression], &mut frames, &crate::EMPTY_DEVICE_RESOURCES),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert_eq!(frames, unchanged);
    pair.rollback_block();
    assert!(pair.committed_is_quiescent());
}

struct PairRetargetProof {
    target_key: u8,
    target_value: f32,
}

impl PreparedInstrumentHistoryRetarget for PairRetargetProof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct PairRetargetPrepared {
    key: u8,
    reject_second_twin: bool,
    initial_value: f32,
    creations: Arc<AtomicUsize>,
}

struct PairRetargetRuntime {
    key: u8,
    reject_retarget: bool,
    value: f32,
}

impl PreparedMutableInstrumentRuntime for PairRetargetPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<PairRetargetRuntime>()
    }

    fn max_events_per_block(&self) -> usize {
        8
    }

    fn max_active_occurrences(&self) -> usize {
        2
    }

    fn max_release_tail_frames(&self) -> u64 {
        0
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let ordinal = self.creations.fetch_add(1, Ordering::Relaxed);
        Box::new(PairRetargetRuntime {
            key: self.key,
            reject_retarget: self.reject_second_twin && ordinal == 1,
            value: self.initial_value,
        })
    }

    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            PairRetargetProof {
                target_key: self.key,
                target_value: 7.0,
            },
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for PairRetargetRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _frame_count: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.is_empty() {
            Ok(())
        } else {
            Err(NativeInstrumentRuntimeError::InvalidEventBatch)
        }
    }

    fn process_block(
        &mut self,
        _events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            frame[0] += self.value;
        }
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.key == previous.key)
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.value = previous.value;
        }
    }

    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        !self.reject_retarget
            && proof
                .as_any()
                .downcast_ref::<PairRetargetProof>()
                .is_some_and(|proof| {
                    proof.target_key == self.key
                        && previous
                            .as_any()
                            .downcast_ref::<Self>()
                            .is_some_and(|previous| previous.key == 1)
                })
    }

    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        let Some(proof) = proof.as_any().downcast_ref::<PairRetargetProof>() else {
            return;
        };
        if self.can_retarget_history_from(previous, proof) {
            self.value = proof.target_value;
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn is_quiescent(&self) -> bool {
        self.value == 0.0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[test]
fn retarget_preflights_both_twins_before_mutating_either() {
    let old_prepared = PairRetargetPrepared {
        key: 1,
        reject_second_twin: false,
        initial_value: 3.0,
        creations: Arc::new(AtomicUsize::new(0)),
    };
    let target_prepared = PairRetargetPrepared {
        key: 2,
        reject_second_twin: true,
        initial_value: 0.0,
        creations: Arc::new(AtomicUsize::new(0)),
    };
    let previous = NativeInstrumentRuntimePair::new(&old_prepared, TEST_LIMITS).unwrap();
    let mut target = NativeInstrumentRuntimePair::new(&target_prepared, TEST_LIMITS).unwrap();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target_prepared.prepare_history_retarget_target().unwrap()
    else {
        panic!("target provides explicit retarget proof");
    };

    assert!(!target.can_retarget_from(&previous, proof.as_ref()));
    assert_eq!(
        target.retarget_from(&previous, proof.as_ref()),
        Err(NativeInstrumentRuntimeError::IncompatibleHistory)
    );
    for runtime in [&target.primary, &target.scratch] {
        assert_eq!(
            runtime
                .as_any()
                .downcast_ref::<PairRetargetRuntime>()
                .unwrap()
                .value,
            0.0,
            "failed second-twin preflight leaves both targets unchanged"
        );
    }

    // The source isn't mutated by failed candidate preflight either.
    assert_eq!(
        previous
            .primary
            .as_any()
            .downcast_ref::<PairRetargetRuntime>()
            .unwrap()
            .value,
        3.0
    );
}

fn event_off(offset: usize, id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset: offset,
        occurrence_id: id,
    }
}

fn new_pair(key: u8, release_frames: u8) -> NativeInstrumentRuntimePair {
    NativeInstrumentRuntimePair::new(
        &prepared(key, release_frames, Arc::new(AtomicUsize::new(0))),
        TEST_LIMITS,
    )
    .unwrap()
}

fn new_pair_with_voices(
    key: u8,
    release_frames: u8,
    max_voices: usize,
) -> NativeInstrumentRuntimePair {
    let mut prepared = prepared(key, release_frames, Arc::new(AtomicUsize::new(0)));
    prepared.max_voices = max_voices;
    NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).unwrap()
}

fn render(
    runtime: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    frame_count: usize,
) -> Vec<StereoFrame> {
    let mut frames = vec![[0.0, 0.0]; frame_count];
    runtime.begin_block();
    runtime
        .process_block(events, &mut frames, &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    runtime.commit_block();
    frames
}

#[test]
fn default_capability_stays_off_and_mutable_only_does_not_fake_analytic_audio() {
    struct AnalyticOnly;
    impl PreparedNativeInstrument for AnalyticOnly {
        fn prepare_note(
            &self,
            _pitch: u8,
            _velocity: u8,
        ) -> Result<PreparedAnalyticNote, DeviceContractError> {
            Ok(PreparedAnalyticNote::new(
                [0; ANALYTIC_NOTE_PAYLOAD_WORDS],
                0,
                |_, _, _, _, _| [0.0, 0.0],
            ))
        }
    }

    struct MutableOnly(FeedbackPrepared);
    impl PreparedNativeInstrument for MutableOnly {
        fn prepare_note(
            &self,
            _pitch: u8,
            _velocity: u8,
        ) -> Result<PreparedAnalyticNote, DeviceContractError> {
            Err(DeviceContractError::UnsupportedProcessingClass)
        }

        fn prepare_mutable_runtime(
            &self,
        ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError>
        {
            Ok(Some(Box::new(FeedbackPrepared {
                observation: self.0.observation,
                compatibility_key: self.0.compatibility_key,
                release_frames: self.0.release_frames,
                max_tail_frames: self.0.max_tail_frames,
                max_events: self.0.max_events,
                max_voices: self.0.max_voices,
                declared_bytes: self.0.declared_bytes,
                creations: Arc::clone(&self.0.creations),
                alternate_twin_key: self.0.alternate_twin_key,
            })))
        }
    }

    let analytic = AnalyticOnly;
    assert!(analytic.prepare_mutable_runtime().unwrap().is_none());

    let mutable = MutableOnly(prepared(1, 2, Arc::new(AtomicUsize::new(0))));
    assert_eq!(
        mutable.prepare_note(60, 100).unwrap_err(),
        DeviceContractError::UnsupportedProcessingClass
    );
    let factory = mutable.prepare_mutable_runtime().unwrap().unwrap();
    let pair = NativeInstrumentRuntimePair::new(factory.as_ref(), TEST_LIMITS).unwrap();
    assert_eq!(
        pair.fixed_storage_pair_bytes(),
        Some(pair.declared_pair_bytes())
    );
}

#[test]
fn overlapping_same_pitch_occurrences_keep_distinct_ids_and_partition_exactly() {
    let mut whole = new_pair(1, 2);
    let whole_pcm = render(
        &mut whole,
        &[
            event_on(0, 11, 60, 83),
            event_on(0, 12, 60, 117),
            event_off(3, 11),
            event_off(5, 12),
        ],
        8,
    );

    let mut partitioned = new_pair(1, 2);
    let mut partitioned_pcm = Vec::new();
    partitioned_pcm.extend(render(
        &mut partitioned,
        &[event_on(0, 11, 60, 83), event_on(0, 12, 60, 117)],
        3,
    ));
    partitioned_pcm.extend(render(&mut partitioned, &[event_off(0, 11)], 2));
    partitioned_pcm.extend(render(&mut partitioned, &[event_off(0, 12)], 3));
    assert_eq!(whole_pcm, partitioned_pcm);
    assert!(whole_pcm.iter().any(|frame| *frame != [0.0, 0.0]));
}

#[test]
fn instrument_adds_to_existing_chain_audio_without_overwriting_it() {
    let events = [event_on(0, 31, 64, 113)];
    let mut generated_only = new_pair(1, 2);
    let generated = render(&mut generated_only, &events, 5);

    let mut mixed = new_pair(1, 2);
    let mut chain_input = [[0.25, -0.5]; 5];
    mixed.begin_block();
    mixed
        .process_block(&events, &mut chain_input, &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    mixed.commit_block();
    let expected: [[f32; 2]; 5] =
        std::array::from_fn(|index| [0.25 + generated[index][0], -0.5 + generated[index][1]]);
    assert_eq!(chain_input, expected);
}

#[test]
fn same_frame_off_then_on_order_frees_capacity_before_the_next_event() {
    let mut one_voice = prepared(1, 0, Arc::new(AtomicUsize::new(0)));
    one_voice.max_voices = 1;
    let mut runtime = NativeInstrumentRuntimePair::new(
        &one_voice,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 4_096,
            max_events_per_block: 4,
            max_active_occurrences: 1,
            max_tail_frames: 16,
        },
    )
    .unwrap();
    render(&mut runtime, &[event_on(0, 1, 60, 100)], 2);

    runtime.begin_block();
    let mut off_then_on = [[0.0, 0.0]; 2];
    runtime
        .process_block(
            &[event_off(0, 1), event_on(0, 2, 60, 100)],
            &mut off_then_on,
            &crate::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    runtime.commit_block();

    runtime.begin_block();
    let mut on_then_off = [[7.0, -3.0]; 2];
    let unchanged = on_then_off;
    assert_eq!(
        runtime.process_block(
            &[event_on(0, 3, 60, 100), event_off(0, 2)],
            &mut on_then_off,
            &crate::EMPTY_DEVICE_RESOURCES,
        ),
        Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
    );
    assert_eq!(on_then_off, unchanged);
    runtime.rollback_block();
}

#[test]
fn prior_block_release_advances_before_first_event_offset() {
    let mut one_voice = new_pair_with_voices(1, 4, 1);
    let mut two_voice_reference = new_pair_with_voices(1, 4, 2);
    for runtime in [&mut one_voice, &mut two_voice_reference] {
        render(runtime, &[event_on(0, 1, 60, 100)], 2);
        render(runtime, &[event_off(0, 1)], 1);
    }

    let candidate = event_on(3, 2, 64, 90);
    let actual = render(&mut one_voice, &[candidate], 4);
    let reference = render(&mut two_voice_reference, &[candidate], 4);
    assert_eq!(actual, reference);
    assert!(actual.iter().any(|frame| *frame != [0.0, 0.0]));
}

#[test]
fn event_only_preflight_checks_generic_bounds_and_both_histories_without_processing() {
    let mut runtime = new_pair(1, 2);
    let on = [event_on(0, 11, 60, 100)];
    assert!(
        runtime
            .preflight_event_batch(&on, 1, &crate::EMPTY_DEVICE_RESOURCES)
            .is_ok()
    );
    assert!(runtime.committed_is_quiescent());
    assert_eq!(
        runtime.preflight_event_batch(
            &[event_on(1, 11, 60, 100)],
            1,
            &crate::EMPTY_DEVICE_RESOURCES
        ),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert_eq!(
        runtime.preflight_event_batch(&[on[0]; 9], 1, &crate::EMPTY_DEVICE_RESOURCES),
        Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
    );
    runtime.begin_block();
    runtime
        .process_block(&on, &mut [[0.0; 2]], &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    // Only scratch has the active ID; a primary-only check would accept it.
    assert_eq!(
        runtime.preflight_event_batch(&on, 1, &crate::EMPTY_DEVICE_RESOURCES),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    runtime.rollback_block();
    assert!(runtime.committed_is_quiescent());
    runtime.begin_block();
    assert!(
        runtime
            .preflight_event_batch(&on, 1, &crate::EMPTY_DEVICE_RESOURCES)
            .is_ok()
    );
    runtime
        .process_block(&on, &mut [[0.0; 2]], &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    runtime.commit_block();
    // Commit swaps twins: only primary now has the active ID.
    assert_eq!(
        runtime.preflight_event_batch(&on, 1, &crate::EMPTY_DEVICE_RESOURCES),
        Err(NativeInstrumentRuntimeError::InvalidEventBatch)
    );
    assert!(!runtime.committed_is_quiescent());
}

#[test]
fn malformed_or_exhausted_batches_leave_pcm_and_committed_history_unchanged() {
    let mut runtime = new_pair(1, 2);
    let initial = render(&mut runtime, &[event_on(0, 1, 61, 100)], 2);

    for (events, error) in [
        (
            vec![event_on(0, 1, 61, 100)],
            NativeInstrumentRuntimeError::InvalidEventBatch,
        ),
        (
            vec![event_on(1, 2, 60, 90), event_on(0, 3, 60, 90)],
            NativeInstrumentRuntimeError::InvalidEventBatch,
        ),
        (
            vec![event_on(0, 2, 60, 90), event_on(0, 2, 62, 90)],
            NativeInstrumentRuntimeError::InvalidEventBatch,
        ),
        (
            vec![
                event_on(0, 2, 60, 90),
                event_on(0, 3, 62, 90),
                event_off(1, 1),
            ],
            NativeInstrumentRuntimeError::EventCapacityExceeded,
        ),
    ] {
        runtime.begin_block();
        let mut untouched = [[4.0, -2.0]; 3];
        let before = untouched;
        assert_eq!(
            runtime.process_block(&events, &mut untouched, &crate::EMPTY_DEVICE_RESOURCES),
            Err(error)
        );
        assert_eq!(untouched, before);
        runtime.rollback_block();
    }

    let retry = render(&mut runtime, &[], 3);
    let mut reference = new_pair(1, 2);
    let reference_initial = render(&mut reference, &[event_on(0, 1, 61, 100)], 2);
    assert_eq!(initial, reference_initial);
    assert_eq!(retry, render(&mut reference, &[], 3));
}

#[test]
fn speculative_history_rolls_back_and_committed_twins_advance_together() {
    let mut runtime = new_pair(1, 2);
    render(&mut runtime, &[event_on(0, 7, 65, 112)], 3);

    runtime.begin_block();
    let mut failed = [[0.0, 0.0]; 4];
    runtime
        .process_block(
            &[event_off(1, 7)],
            &mut failed,
            &crate::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    runtime.rollback_block();

    let retry = render(&mut runtime, &[], 4);
    let mut reference = new_pair(1, 2);
    render(&mut reference, &[event_on(0, 7, 65, 112)], 3);
    assert_eq!(retry, render(&mut reference, &[], 4));
}

#[test]
fn analytic_import_requires_provider_opt_in_and_leaves_default_runtime_unchanged() {
    let mut target = new_pair(1, 2);
    assert!(!target.can_import_analytic(&[]));
    assert_eq!(
        target.import_analytic(&[]),
        Err(NativeInstrumentRuntimeError::IncompatibleHistory)
    );
    assert_eq!(render(&mut target, &[event_on(0, 9, 67, 120)], 4), {
        let mut fresh = new_pair(1, 2);
        render(&mut fresh, &[event_on(0, 9, 67, 120)], 4)
    });
}

#[test]
fn incompatible_history_is_refused_without_copy_and_candidate_remains_reset() {
    let mut previous = new_pair(1, 2);
    render(&mut previous, &[event_on(0, 9, 67, 120)], 4);
    let mut candidate = new_pair(2, 2);
    assert!(!candidate.can_inherit_from(&previous));
    assert_eq!(
        candidate.inherit_from(&previous),
        Err(NativeInstrumentRuntimeError::IncompatibleHistory)
    );
    assert_eq!(render(&mut candidate, &[event_on(0, 9, 67, 120)], 4), {
        let mut fresh = new_pair(2, 2);
        render(&mut fresh, &[event_on(0, 9, 67, 120)], 4)
    });
}

#[test]
fn different_voice_capacity_is_incompatible_history() {
    let mut previous = new_pair_with_voices(1, 2, 2);
    render(
        &mut previous,
        &[event_on(0, 40, 60, 100), event_on(0, 41, 64, 90)],
        2,
    );
    let mut candidate = new_pair_with_voices(1, 2, 1);
    assert!(!candidate.can_inherit_from(&previous));
    assert_eq!(
        candidate.inherit_from(&previous),
        Err(NativeInstrumentRuntimeError::IncompatibleHistory)
    );
    assert_eq!(render(&mut candidate, &[], 3), [[0.0, 0.0]; 3]);
}

#[test]
fn caller_ceilings_and_twin_arithmetic_are_checked_before_runtime_creation() {
    let creations = Arc::new(AtomicUsize::new(0));
    let prepared_runtime = prepared(1, 1, Arc::clone(&creations));
    let too_small = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 1,
        ..TEST_LIMITS
    };
    assert!(matches!(
        NativeInstrumentRuntimePair::new(&prepared_runtime, too_small),
        Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    ));
    assert_eq!(creations.load(Ordering::Relaxed), 0);

    let mut overflowing = prepared(1, 1, Arc::clone(&creations));
    overflowing.declared_bytes = usize::MAX;
    assert!(matches!(
        NativeInstrumentRuntimePair::new(&overflowing, TEST_LIMITS),
        Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    ));
    assert_eq!(creations.load(Ordering::Relaxed), 0);

    let pair = NativeInstrumentRuntimePair::new(&prepared_runtime, TEST_LIMITS).unwrap();
    assert_eq!(creations.load(Ordering::Relaxed), 2);
    assert_eq!(
        pair.declared_pair_bytes(),
        2 * std::mem::size_of::<FeedbackRuntime>()
    );
    assert_eq!(
        pair.fixed_storage_pair_bytes(),
        Some(pair.declared_pair_bytes())
    );
}

#[test]
fn hard_reset_clears_committed_and_scratch_voices_without_chase() {
    let mut runtime = new_pair(1, 2);
    render(&mut runtime, &[event_on(0, 23, 72, 127)], 4);
    runtime.reset();
    assert_eq!(render(&mut runtime, &[], 4), [[0.0, 0.0]; 4]);
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn release_and_feedback_tail_become_quiescent_within_declared_bound() {
    let mut runtime = new_pair(1, 2);
    assert_eq!(runtime.max_release_tail_frames(), 6);
    assert!(runtime.committed_is_quiescent());
    runtime.begin_block();
    assert!(!runtime.committed_is_quiescent());
    runtime.rollback_block();

    render(&mut runtime, &[event_on(0, 51, 72, 127)], 2);
    assert!(!runtime.committed_is_quiescent());
    // The sample at the NoteOff offset is included in the bound.
    render(&mut runtime, &[event_off(0, 51)], 1);
    assert!(!runtime.committed_is_quiescent());

    let tail = render(&mut runtime, &[], 5);
    assert!(tail.iter().any(|frame| *frame != [0.0, 0.0]));
    assert!(runtime.committed_is_quiescent());
    assert_eq!(render(&mut runtime, &[], 2), [[0.0, 0.0]; 2]);

    let mut zero_release = new_pair(1, 0);
    assert_eq!(zero_release.max_release_tail_frames(), 4);
    render(&mut zero_release, &[event_on(0, 52, 72, 127)], 2);
    render(&mut zero_release, &[event_off(0, 52)], 1);
    assert!(
        render(&mut zero_release, &[], 3)
            .iter()
            .any(|frame| *frame != [0.0, 0.0])
    );
    assert!(zero_release.committed_is_quiescent());
}

#[test]
fn caller_tail_ceiling_is_checked_before_runtime_creation() {
    let creations = Arc::new(AtomicUsize::new(0));
    let prepared = prepared(1, 2, Arc::clone(&creations));
    let too_short = NativeInstrumentRuntimeLimits {
        max_tail_frames: 5,
        ..TEST_LIMITS
    };
    assert_eq!(
        NativeInstrumentRuntimePair::new(&prepared, too_short).err(),
        Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    );
    assert_eq!(creations.load(Ordering::Relaxed), 0);
}

#[test]
fn incompatible_factory_twins_are_refused_during_control_construction() {
    let creations = Arc::new(AtomicUsize::new(0));
    let mut prepared = prepared(1, 1, Arc::clone(&creations));
    prepared.alternate_twin_key = true;
    assert_eq!(
        NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).err(),
        Some(NativeInstrumentRuntimeError::IncompatibleTwins)
    );
    assert_eq!(creations.load(Ordering::Relaxed), 2);
}

#[test]
fn event_and_occurrence_ceilings_are_checked_before_runtime_creation() {
    let creations = Arc::new(AtomicUsize::new(0));
    let prepared = prepared(1, 1, Arc::clone(&creations));
    let no_events = NativeInstrumentRuntimeLimits {
        max_events_per_block: 3,
        ..TEST_LIMITS
    };
    assert_eq!(
        NativeInstrumentRuntimePair::new(&prepared, no_events).err(),
        Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    );
    let one_occurrence = NativeInstrumentRuntimeLimits {
        max_active_occurrences: 1,
        ..TEST_LIMITS
    };
    assert_eq!(
        NativeInstrumentRuntimePair::new(&prepared, one_occurrence).err(),
        Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    );
    assert_eq!(creations.load(Ordering::Relaxed), 0);
}

#[test]
fn underreported_fixed_storage_is_refused_after_bounded_control_construction() {
    let creations = Arc::new(AtomicUsize::new(0));
    let mut prepared = prepared(1, 1, Arc::clone(&creations));
    prepared.declared_bytes -= 1;
    assert_eq!(
        NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).err(),
        Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
    );
    assert_eq!(creations.load(Ordering::Relaxed), 2);
}
#[test]
fn unsupported_occurrence_gain_refuses_before_output_or_speculative_history_changes() {
    let mut runtime = new_pair(1, 2);
    assert!(!runtime.supports_occurrence_gain());
    let gain =
        crate::NativeInstrumentOccurrenceGain::new(1, 0.0, crate::INSTRUMENT_GAIN_UNITY, 4, 0)
            .unwrap();
    let events = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 127,
    }];
    let mut output = [[9.0; 2]; 4];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block_with_clock_and_occurrence_gains(
            &events,
            &mut output,
            None,
            &[gain],
            &crate::EMPTY_DEVICE_RESOURCES
        ),
        Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain)
    );
    assert_eq!(output, [[9.0; 2]; 4]);
    runtime.rollback_block();
    assert!(runtime.committed_is_quiescent());
    // Empty context is the existing processing law, even for providers
    // that cannot apply independent occurrence gains.
    runtime.begin_block();
    let mut actual = [[0.0; 2]; 4];
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &events,
            &mut actual,
            None,
            &[],
            &crate::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(actual.as_slice(), render(&mut new_pair(1, 2), &events, 4));
}

#[test]
fn gain_capability_declaration_must_match_actual_twins() {
    struct ClaimedGain(FeedbackPrepared);
    impl PreparedMutableInstrumentRuntime for ClaimedGain {
        fn supports_occurrence_gain(&self) -> bool {
            true
        }
        fn runtime_version(&self) -> u32 {
            self.0.runtime_version()
        }
        fn declared_twin_bytes(&self) -> usize {
            self.0.declared_twin_bytes()
        }
        fn max_events_per_block(&self) -> usize {
            self.0.max_events_per_block()
        }
        fn max_active_occurrences(&self) -> usize {
            self.0.max_active_occurrences()
        }
        fn max_release_tail_frames(&self) -> u64 {
            self.0.max_release_tail_frames()
        }
        fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
            self.0.create_runtime()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    let factory = ClaimedGain(prepared(1, 2, Arc::new(AtomicUsize::new(0))));
    assert!(matches!(
        NativeInstrumentRuntimePair::new(&factory, TEST_LIMITS),
        Err(NativeInstrumentRuntimeError::IncompatibleTwins)
    ));
}
