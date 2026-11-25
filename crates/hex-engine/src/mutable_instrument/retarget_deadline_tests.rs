//! Owner regressions for retained-note release deadline reconciliation across
//! prepared instrument retargets (Pulse #6833).
use super::*;
use hex_lattice::{
    DeviceResources, EMPTY_DEVICE_RESOURCES, InstrumentHistoryRetargetPreparation,
    MutableInstrumentRuntime, NativeInstrumentEvent, NativeInstrumentOccurrenceGain,
    NativeInstrumentRuntimeError, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime, StereoFrame,
};
use hex_project::{Command, Created, Editor, Project};
use std::any::Any;

const RATE: u32 = 8_000;

#[derive(Clone, Copy, Debug)]
struct RetargetableFactory {
    release_frames: u64,
    max_active: usize,
    steals_releasing: bool,
    tag: u32,
}

#[derive(Clone, Copy, Debug)]
struct RetargetProof {
    release_frames: u64,
    max_active: usize,
    tag: u32,
}

impl PreparedInstrumentHistoryRetarget for RetargetProof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    held: bool,
    release_elapsed: u64,
}

#[derive(Clone)]
struct RetargetableRuntime {
    release_frames: u64,
    max_active: usize,
    tag: u32,
    voices: [Voice; MAX_MUTABLE_OCCURRENCE_PROOFS],
}

impl PreparedMutableInstrumentRuntime for RetargetableFactory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<RetargetableRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        MAX_MUTABLE_EVENTS_PER_QUERY
    }
    fn max_active_occurrences(&self) -> usize {
        self.max_active
    }
    fn steals_releasing_voices(&self) -> bool {
        self.steals_releasing
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release_frames
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(RetargetableRuntime {
            release_frames: self.release_frames,
            max_active: self.max_active,
            tag: self.tag,
            voices: [Voice::default(); MAX_MUTABLE_OCCURRENCE_PROOFS],
        })
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, hex_lattice_contract::DeviceContractError>
    {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            RetargetProof {
                release_frames: self.release_frames,
                max_active: self.max_active,
                tag: self.tag,
            },
        )))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for RetargetableRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn preflight_occurrence_gains(
        &self,
        _events: &[NativeInstrumentEvent],
        _gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let count = self.voices.iter().filter(|v| v.id != 0).count();
        if count > output.len() {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        for (dst, voice) in output
            .iter_mut()
            .zip(self.voices.iter().filter(|v| v.id != 0))
        {
            *dst = voice.id;
        }
        Ok(count)
    }
    fn preflight_events(
        &self,
        _events: &[NativeInstrumentEvent],
        _frame_count: usize,
        _resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        output: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, output, None, &[], resources);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        output: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        occurrence_gains: &[NativeInstrumentOccurrenceGain],
        _resources: &dyn DeviceResources,
    ) {
        output.fill([0.0; 2]);
        let mut cursor = 0usize;
        for (frame_index, frame) in output.iter_mut().enumerate() {
            while cursor < events.len() && events[cursor].frame_offset() == frame_index {
                match events[cursor] {
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if let Some(voice) = self
                            .voices
                            .iter_mut()
                            .find(|voice| voice.id == occurrence_id)
                        {
                            voice.held = false;
                            voice.release_elapsed = 0;
                            if self.release_frames == 0 {
                                *voice = Voice::default();
                            }
                        }
                    }
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        let slot_idx = self
                            .voices
                            .iter()
                            .take(self.max_active)
                            .position(|voice| voice.id == 0)
                            .or_else(|| {
                                self.voices
                                    .iter()
                                    .take(self.max_active)
                                    .position(|voice| !voice.held)
                            });
                        if let Some(slot_idx) = slot_idx {
                            self.voices[slot_idx] = Voice {
                                id: occurrence_id,
                                held: true,
                                release_elapsed: 0,
                            };
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            let mut sum = 0.0_f32;
            for voice in &mut self.voices {
                if voice.id == 0 {
                    continue;
                }
                if !voice.held && voice.release_elapsed >= self.release_frames {
                    *voice = Voice::default();
                    continue;
                }
                let gain = occurrence_gains
                    .iter()
                    .find(|g| g.occurrence_id() == voice.id)
                    .map_or(1.0_f32, |g| g.value_at_offset(frame_index as u64) as f32);
                sum += 0.25 * gain;
                if !voice.held {
                    voice.release_elapsed += 1;
                    if voice.release_elapsed >= self.release_frames {
                        *voice = Voice::default();
                    }
                }
            }
            *frame = [sum; 2];
        }
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
    }
    fn reset(&mut self) {
        self.voices = [Voice::default(); MAX_MUTABLE_OCCURRENCE_PROOFS];
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            old.release_frames == self.release_frames
                && old.max_active == self.max_active
                && old.tag == self.tag
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        let Some(proof) = proof.as_any().downcast_ref::<RetargetProof>() else {
            return false;
        };
        let active = old.voices.iter().filter(|v| v.id != 0).count();
        proof.release_frames == self.release_frames
            && proof.max_active == self.max_active
            && proof.tag == self.tag
            && active <= self.max_active
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        _proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.voices = [Voice::default(); MAX_MUTABLE_OCCURRENCE_PROOFS];
        let mut dst = 0usize;
        for voice in old.voices.iter().copied().filter(|v| v.id != 0) {
            if !voice.held && voice.release_elapsed >= self.release_frames {
                continue;
            }
            if dst < self.max_active {
                self.voices[dst] = voice;
                dst += 1;
            }
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn fixture_ids() -> (TrackId, ClipId, NoteId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "retarget".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    (track, clip, note)
}

fn make_onset(
    track: TrackId,
    clip: ClipId,
    note: NoteId,
    iteration: u64,
    loop_pass: u64,
    start: u64,
    gate_end: u64,
) -> MutableNoteOnset {
    MutableNoteOnset {
        key: MutableOccurrenceKey::new(track, clip, note, iteration, loop_pass),
        onset_beat: BeatTime::from_raw(start),
        gate_end_beat: BeatTime::from_raw(gate_end),
        source_start: start,
        source_gate_end: gate_end,
        pitch: 60,
        channel: 0,
        velocity: 100,
        contributor_fade_in: false,
        expression: Default::default(),
    }
}

fn make_runtime(
    release_frames: u64,
    max_active: usize,
    steals_releasing: bool,
    shared_source: bool,
    tag: u32,
) -> (
    MutableInstrumentTrackRuntime,
    Box<dyn PreparedInstrumentHistoryRetarget>,
) {
    let factory = RetargetableFactory {
        release_frames,
        max_active,
        steals_releasing,
        tag,
    };
    let mut runtime = MutableInstrumentTrackRuntime::new(&factory, RATE).unwrap();
    if shared_source {
        runtime.live = Some(Box::new(live::LiveLedger::default()));
    }
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    (runtime, proof)
}

fn unity_volume(
    _: MutableOccurrenceSignature,
    at: u64,
    _: usize,
    _: bool,
) -> Result<Option<volume::VolumeSlice>, MutableInstrumentRuntimeError> {
    Ok(Some(volume::VolumeSlice {
        at,
        ramp: crate::midi_clip_volume::VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 1.0,
            end_gain: 1.0,
            next_boundary: None,
        },
        terminal: None,
    }))
}

#[test]
fn lengthening_release_reconciles_held_and_released_deadlines_through_new_terminal() {
    let (track, clip, note) = fixture_ids();
    for shared_source in [false, true] {
        let (mut old, _) = make_runtime(8, 4, false, shared_source, 1);
        let mut onsets = MutableOnsetBuffer::default();
        // Note 1: starts at 0, releases at 4 (already released before retarget at frame 8).
        // Note 2: starts at 2, releases at 12 (still held at retarget at frame 8).
        onsets
            .push(make_onset(track, clip, note, 0, 0, 0, 4))
            .unwrap();
        onsets
            .push(make_onset(track, clip, note, 1, 0, 2, 12))
            .unwrap();
        let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
        let mut out = [[0.0; 2]; 8];
        old.begin_block();
        if shared_source {
            let mut lookup = unity_volume;
            old.process_source_segment(
                0,
                &mut out,
                &mut onsets,
                &[],
                &mut events,
                None,
                &EMPTY_DEVICE_RESOURCES,
                Some(&mut lookup),
                None,
            )
            .unwrap();
        } else {
            old.process_segment(track, 0, 0, &mut out, &mut onsets, &[], &mut events)
                .unwrap();
        }
        old.commit_block();
        onsets.clear();

        let before: Vec<_> = old.proofs().collect();
        assert_eq!(before.len(), 2);
        assert!(before[0].note_off_sent);
        assert_eq!(before[0].quiet_deadline_frame, 4 + 8);
        assert!(!before[1].note_off_sent);
        assert_eq!(before[1].quiet_deadline_frame, 12 + 8);
        if shared_source {
            assert_eq!(before[0].release_origin, Some(ReleaseOrigin::AtAge(4)));
            assert_eq!(before[0].quiet_deadline_age, Some(12));
            assert_eq!(before[1].release_origin, None);
            assert_eq!(before[1].quiet_deadline_age, None);
        }

        // Retarget to a much longer release (40 frames) at performance frame 8.
        // Without reconciliation, note 1's proof would be removed at frame 12 and
        // note 2's proof would be removed at frame 20 while DSP is audible until
        // frames 44 and 52 respectively (failing volume gain lookup on stale proofs).
        let (mut next, proof) = make_runtime(40, 4, false, shared_source, 2);
        assert!(next.can_retarget_from(&old, proof.as_ref()));
        next.retarget_from(&old, proof.as_ref()).unwrap();

        let after: Vec<_> = next.proofs().collect();
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].id, before[0].id);
        assert_eq!(after[1].id, before[1].id);
        assert_eq!(after[0].quiet_deadline_frame, 4 + 40);
        assert_eq!(after[1].quiet_deadline_frame, 12 + 40);
        if shared_source {
            assert_eq!(after[0].quiet_deadline_age, Some(4 + 40));
            assert_eq!(after[1].quiet_deadline_age, None);
        }

        // Render frame-by-frame through frame 56 (past both old deadlines 12/20 and new terminals 44/52),
        // exercising volume gain proof resolution on every single frame plus a rollback at frame 16.
        let mut rolled_back = false;
        let mut frame = 8u64;
        while frame < 56 {
            let mut single = [[0.0; 2]; 1];
            next.begin_block();
            let mut lookup = unity_volume;
            if shared_source {
                next.process_source_segment(
                    frame,
                    &mut single,
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &EMPTY_DEVICE_RESOURCES,
                    Some(&mut lookup),
                    None,
                )
                .unwrap();
            } else {
                next.process_segment_with_resources(
                    track,
                    frame,
                    frame,
                    &mut single,
                    &mut onsets,
                    &[],
                    &mut events,
                    None,
                    &EMPTY_DEVICE_RESOURCES,
                    None,
                )
                .unwrap();
            }
            if !rolled_back && frame == 16 {
                next.rollback_block();
                rolled_back = true;
                continue;
            }
            next.commit_block();
            // Note 1 audible on [0, 44), Note 2 audible on [2, 52).
            let expected_voices = usize::from(frame < 44) + usize::from(frame < 52);
            assert_eq!(
                single[0][0],
                0.25 * expected_voices as f32,
                "shared={shared_source} frame={frame}"
            );
            assert_eq!(
                next.committed_is_quiescent(),
                frame + 1 >= 52,
                "shared={shared_source} frame={frame}"
            );
            frame += 1;
        }
        assert!(rolled_back);
        assert_eq!(next.proofs().count(), 0);
    }
}

#[test]
fn live_voice_deadlines_reconcile_across_lengthen_shorten_zero_and_repeated_edits() {
    let (track, _, _) = fixture_ids();
    let input = LiveInputId {
        track,
        generation: 1,
    };
    let (mut current, _) = make_runtime(10, 4, false, true, 1);
    let mut onsets = MutableOnsetBuffer::default();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);

    // Trigger live token 1 at age 0, release at age 4; trigger live token 2 at age 0 (kept held).
    current.begin_block();
    current
        .stage_live_events(
            input,
            &[
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 2,
                    pitch: 64,
                    velocity: 100,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 4,
                    occurrence_id: 1,
                },
            ],
            None,
        )
        .unwrap();
    let mut block = [[0.0; 2]; 8];
    current
        .process_source_segment(
            0,
            &mut block,
            &mut onsets,
            &[],
            &mut events,
            None,
            &EMPTY_DEVICE_RESOURCES,
            None,
            None,
        )
        .unwrap();
    current.commit_block();
    let live_ids: Vec<u64> = current
        .live
        .as_ref()
        .unwrap()
        .primary
        .iter()
        .flatten()
        .map(|v| v.id)
        .collect();
    assert_eq!(live_ids, [1, 2]);
    assert_eq!(
        current.live.as_ref().unwrap().primary[0].unwrap().quiet_age,
        Some(14)
    );
    assert_eq!(
        current.live.as_ref().unwrap().primary[1].unwrap().quiet_age,
        None
    );

    // Repeated edits without rendering in between: lengthen (10 -> 50), shorten (50 -> 6),
    // zero (6 -> 0), then lengthen again (0 -> 30). ReleaseOrigin keeps every edit exact!
    for (tail, expected_quiet, tag) in [(50, 54, 2), (6, 10, 3), (0, 4, 4), (30, 34, 5)] {
        let (mut next, proof) = make_runtime(tail, 4, false, true, tag);
        assert!(next.can_retarget_from(&current, proof.as_ref()));
        next.retarget_from(&current, proof.as_ref()).unwrap();
        let v0 = next.live.as_ref().unwrap().primary[0].unwrap();
        let v1 = next.live.as_ref().unwrap().primary[1].unwrap();
        assert_eq!(v0.id, 1);
        assert_eq!(v1.id, 2);
        assert_eq!(v0.release_origin, Some(ReleaseOrigin::AtAge(4)));
        assert_eq!(v0.quiet_age, Some(expected_quiet));
        assert_eq!(v1.quiet_age, None);
        current = next;
    }

    // Re-shorten to 0 (quiet_age = 4 <= current age 8) and release held voice 2 at age 8:
    // voice 1 retires cleanly and voice 2 finishes immediately with zero tail.
    let (mut zero, proof) = make_runtime(0, 4, false, true, 6);
    zero.retarget_from(&current, proof.as_ref()).unwrap();
    zero.begin_block();
    zero.stage_live_events(
        input,
        &[NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 2,
        }],
        None,
    )
    .unwrap();
    let mut tail_block = [[0.0; 2]; 4];
    zero.process_source_segment(
        8,
        &mut tail_block,
        &mut onsets,
        &[],
        &mut events,
        None,
        &EMPTY_DEVICE_RESOURCES,
        None,
        None,
    )
    .unwrap();
    zero.commit_block();
    assert_eq!(tail_block, [[0.0; 2]; 4]);
    assert!(zero.committed_is_quiescent());
    assert_eq!(zero.transition_live_count(false), 0);
}

#[test]
fn extended_release_preserves_capacity_across_loop_wrap_and_cut_gate_boundaries() {
    let (track, clip, note) = fixture_ids();
    // Pass 0 note: [0, 4) with initial release 4 (old quiet deadline = 8, before loop wrap at 12).
    let (mut old, _) = make_runtime(4, 2, false, false, 1);
    let mut onsets = MutableOnsetBuffer::default();
    onsets
        .push(make_onset(track, clip, note, 0, 0, 0, 4))
        .unwrap();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    old.begin_block();
    old.process_segment(
        track,
        0,
        0,
        &mut [[0.0; 2]; 6],
        &mut onsets,
        &[],
        &mut events,
    )
    .unwrap();
    old.commit_block();
    onsets.clear();

    // Lengthen release to 20 at frame 6 -> new quiet deadline = 24 (extends past loop wrap at 12!).
    // A 1-voice non-stealing runtime must refuse pass 1 at frame 12 while pass 0 is still ringing.
    let (mut one_voice_target, proof_one) = make_runtime(20, 1, false, false, 2);
    one_voice_target
        .retarget_from(&old, proof_one.as_ref())
        .unwrap();
    onsets
        .push(make_onset(track, clip, note, 0, 1, 0, 4))
        .unwrap();
    one_voice_target.begin_block();
    assert_eq!(
        one_voice_target.process_segment(
            track,
            12,
            0,
            &mut [[0.0; 2]; 4],
            &mut onsets,
            &[],
            &mut events
        ),
        Err(MutableInstrumentRuntimeError::OccurrenceCapacity)
    );
    one_voice_target.rollback_block();

    // A 2-voice runtime admits pass 1 alongside pass 0's extended tail with distinct IDs.
    let (mut two_voice_target, proof_two) = make_runtime(20, 2, false, false, 3);
    two_voice_target
        .retarget_from(&old, proof_two.as_ref())
        .unwrap();
    two_voice_target.begin_block();
    let mut pass1 = [[0.0; 2]; 4];
    two_voice_target
        .process_segment(track, 12, 0, &mut pass1, &mut onsets, &[], &mut events)
        .unwrap();
    two_voice_target.commit_block();
    let ids: Vec<(u64, u64)> = two_voice_target
        .proofs()
        .map(|p| (p.id, p.quiet_deadline_frame))
        .collect();
    assert_eq!(ids, [(1, 24), (2, 36)]);
    // Both pass 0 tail and pass 1 held note sound together on frames 12..16.
    assert!(pass1.iter().all(|f| *f == [0.5; 2]));

    // Tempo/edit cut boundary: cut a held note's gate at frame 10, then retarget release to 30.
    let (mut cut_source, _) = make_runtime(4, 2, false, false, 10);
    onsets.clear();
    onsets
        .push(make_onset(track, clip, note, 0, 0, 0, 100))
        .unwrap();
    cut_source.begin_block();
    cut_source
        .process_segment(
            track,
            0,
            0,
            &mut [[0.0; 2]; 6],
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    cut_source.commit_block();
    cut_source.retime_held_gates(|_p| Some((10, 10 + 4)));
    assert_eq!(cut_source.proofs().next().unwrap().gate_end_frame, 10);
    assert_eq!(cut_source.proofs().next().unwrap().quiet_deadline_frame, 14);
    let (mut cut_target, cut_proof) = make_runtime(30, 2, false, false, 11);
    cut_target
        .retarget_from(&cut_source, cut_proof.as_ref())
        .unwrap();
    let reconciled = cut_target.proofs().next().unwrap();
    assert_eq!(reconciled.gate_end_frame, 10);
    assert_eq!(reconciled.quiet_deadline_frame, 40);
}

#[test]
fn invalid_overflow_and_capacity_retargets_refuse_atomically_without_mutation() {
    let (track, clip, note) = fixture_ids();

    // 1. Capacity refusal: 2 carried proofs cannot retarget into a 1-voice non-stealing runtime.
    let (mut two, _) = make_runtime(8, 2, false, false, 1);
    let mut onsets = MutableOnsetBuffer::default();
    onsets
        .push(make_onset(track, clip, note, 0, 0, 0, 4))
        .unwrap();
    onsets
        .push(make_onset(track, clip, note, 1, 0, 0, 8))
        .unwrap();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    two.begin_block();
    two.process_segment(
        track,
        0,
        0,
        &mut [[0.0; 2]; 6],
        &mut onsets,
        &[],
        &mut events,
    )
    .unwrap();
    two.commit_block();

    let (mut small, small_proof) = make_runtime(24, 1, false, false, 2);
    assert!(!small.can_retarget_from(&two, small_proof.as_ref()));
    assert_eq!(
        small.retarget_from(&two, small_proof.as_ref()),
        Err(MutableInstrumentRuntimeError::Runtime(
            NativeInstrumentRuntimeError::IncompatibleHistory
        ))
    );
    assert_eq!(small.proofs().count(), 0);
    assert_eq!(small.next_id, 0);

    // 2. Overflow refusal: gate_end_frame + target_tail overflows u64 -> atomic refusal.
    let mut overflow_source = two;
    overflow_source.primary[0].gate_end_frame = u64::MAX - 5;
    let (mut longer, longer_proof) = make_runtime(20, 2, false, false, 3);
    assert!(!longer.can_retarget_from(&overflow_source, longer_proof.as_ref()));
    assert!(
        longer
            .retarget_from(&overflow_source, longer_proof.as_ref())
            .is_err()
    );
    assert_eq!(longer.proofs().count(), 0);

    // 3. Invalid invariant refusal: held note (!note_off_sent) carrying a quiet_deadline_age.
    overflow_source.primary[0].gate_end_frame = 4;
    overflow_source.primary[1].quiet_deadline_age = Some(99);
    assert!(!longer.can_retarget_from(&overflow_source, longer_proof.as_ref()));
    assert!(
        longer
            .retarget_from(&overflow_source, longer_proof.as_ref())
            .is_err()
    );
    assert_eq!(longer.proofs().count(), 0);

    // 4. Live voice overflow + live-ledger mismatch refusal.
    let (mut live_source, _) = make_runtime(8, 2, false, true, 4);
    live_source.live.as_mut().unwrap().primary[0] = Some(live::LiveOccurrence {
        input: LiveInputId {
            track,
            generation: 1,
        },
        token: 1,
        id: 1,
        release_origin: Some(ReleaseOrigin::AtAge(u64::MAX - 3)),
        quiet_age: Some(u64::MAX),
        release_requested: false,
        contributor_fade: None,
    });
    let (mut live_longer, live_proof) = make_runtime(20, 2, false, true, 5);
    assert!(!live_longer.can_retarget_from(&live_source, live_proof.as_ref()));
    assert!(
        live_longer
            .retarget_from(&live_source, live_proof.as_ref())
            .is_err()
    );
    assert_eq!(live_longer.transition_live_count(false), 0);

    // Missing live ledger on target refuses even when deadlines do not overflow.
    live_source.live.as_mut().unwrap().primary[0] = Some(live::LiveOccurrence {
        input: LiveInputId {
            track,
            generation: 1,
        },
        token: 1,
        id: 1,
        release_origin: Some(ReleaseOrigin::AtAge(4)),
        quiet_age: Some(12),
        release_requested: false,
        contributor_fade: None,
    });
    let (mut no_live_target, no_live_proof) = make_runtime(20, 2, false, false, 6);
    assert!(!no_live_target.can_retarget_from(&live_source, no_live_proof.as_ref()));
    assert!(
        no_live_target
            .retarget_from(&live_source, no_live_proof.as_ref())
            .is_err()
    );
}
