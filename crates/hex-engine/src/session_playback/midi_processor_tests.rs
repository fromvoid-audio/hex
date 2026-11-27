use super::*;
use hex_lattice::conformance::midi_transpose::{
    PreparedTransposeMidiProcessor, TransposeOutOfRangePolicy,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;

const RATE: u32 = 48_000;

fn midi_proc_id() -> NonZeroU64 {
    NonZeroU64::new(9_001).unwrap()
}

fn beat(quarter_notes: u64) -> BeatTime {
    BeatTime::from_raw(quarter_notes << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

fn subtractive_short_release_state() -> hex_lattice_contract::NativeDeviceState {
    let base = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::subtractive::SUBTRACTIVE_TYPE_ID,
    )
    .unwrap();
    let short_attack = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &base,
        hex_devices::subtractive::parameter::ATTACK,
        hex_lattice_contract::ParameterValue::Float(0.001),
    )
    .unwrap();
    crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &short_attack,
        hex_devices::subtractive::parameter::RELEASE,
        hex_lattice_contract::ParameterValue::Float(0.005),
    )
    .unwrap()
}

fn build_subtractive_project(notes: &[(BeatTime, BeatTime, u8, u8)]) -> (Editor, TrackId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi-proc-test".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    if editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .is_some()
    {
        editor.execute(Command::RemoveInstrument { track }).unwrap();
    }
    editor
        .execute(Command::InsertInstrument {
            track,
            state: subtractive_short_release_state(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("expected clip");
    };
    for &(start, duration, pitch, velocity) in notes {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 0,
                velocity,
            })
            .unwrap();
    }
    (editor, track)
}

fn prepare_revision(project: &Project) -> Arc<PreparedRevision> {
    let table = Arc::new(hex_devices::SineTable::new());
    Arc::new(PreparedRevision::prepare(project, &BTreeMap::new(), table, None).unwrap())
}

#[test]
fn transpose_route_matches_shifted_pcm_oracle_across_partitions_and_same_pitch_overlap() {
    // Two overlapping C4 (60) notes with distinct lifetimes + an E4 (64) note.
    // At 120 BPM / 48 kHz, 1/32 beat = 750 frames, 1/16 beat = 1500 frames.
    let authored_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 85),
        (beat_frac(1, 16), beat_frac(1, 32), 64, 100),
    ];
    let shifted_notes: Vec<_> = authored_notes
        .iter()
        .map(|&(s, d, p, v)| (s, d, p + 7, v))
        .collect();

    let (source_editor, track) = build_subtractive_project(&authored_notes);
    let (oracle_editor, _) = build_subtractive_project(&shifted_notes);

    let source_rev = prepare_revision(source_editor.project());
    let oracle_rev = prepare_revision(oracle_editor.project());
    let transpose =
        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse).unwrap();

    // Total render window covers all notes + 5 ms (240 frames) release tail with margin.
    let total_frames = 3_000_usize;

    let mut oracle = SessionPlayback::new(Arc::clone(&oracle_rev)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = vec![[0.0_f32; 2]; total_frames];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|s| s.abs() > 1e-3));
    assert!(
        oracle.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Whole-block render with transpose MIDI processor matches oracle bit-for-bit.
    let mut whole = SessionPlayback::new(Arc::clone(&source_rev)).unwrap();
    let before_storage = whole.accounted_runtime_storage_bytes().unwrap();
    let before_declared = whole.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .declared_pair_bytes();
    whole
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose)
        .unwrap();
    let installed_storage = whole.accounted_runtime_storage_bytes().unwrap();
    let processor_pair =
        2 * hex_lattice::PreparedNativeMidiProcessor::declared_twin_bytes(&transpose);
    let output_scratch = size_of::<hex_lattice::NativeInstrumentEvent>()
        * crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY;
    assert!(installed_storage > before_storage + processor_pair + output_scratch);
    assert_eq!(
        whole.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .declared_pair_bytes(),
        before_declared + processor_pair
    );

    whole
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose)
        .unwrap();
    assert_eq!(
        whole.accounted_runtime_storage_bytes(),
        Some(installed_storage)
    );
    let too_many_events = PreparedTransposeMidiProcessor::with_caps(
        7,
        TransposeOutOfRangePolicy::Refuse,
        crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY + 1,
        1,
        1,
        1,
    )
    .unwrap();
    assert!(
        whole
            .install_track_midi_processor_on_control(track, midi_proc_id(), &too_many_events)
            .is_err()
    );
    assert_eq!(
        whole.accounted_runtime_storage_bytes(),
        Some(installed_storage)
    );
    whole.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_whole = vec![[0.0_f32; 2]; total_frames];
    whole.render(&mut actual_whole).unwrap();
    assert_eq!(actual_whole, expected);
    assert!(
        whole.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Irregular block partitions match bit-for-bit and end completely quiescent.
    let mut chunked = SessionPlayback::new(Arc::clone(&source_rev)).unwrap();
    chunked
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose)
        .unwrap();
    chunked.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_chunked = vec![[0.0_f32; 2]; total_frames];
    let pattern = [1, 3, 7, 13, 29, 64, 128, 256];
    let mut cursor = 0_usize;
    let mut step = 0_usize;
    while cursor < total_frames {
        let len = pattern[step % pattern.len()].min(total_frames - cursor);
        chunked
            .render(&mut actual_chunked[cursor..cursor + len])
            .unwrap();
        cursor += len;
        step += 1;
    }
    assert_eq!(actual_chunked, expected);
    assert!(
        chunked.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
}

#[test]
fn stop_seek_loop_wrap_and_session_route_transitions_leave_no_stuck_notes() {
    let authored_notes = [(BeatTime::ZERO, beat_frac(1, 32), 60, 110)];
    let shifted_notes = [(BeatTime::ZERO, beat_frac(1, 32), 67, 110)];

    let (mut source_editor, track) = build_subtractive_project(&authored_notes);
    let (mut oracle_editor, _) = build_subtractive_project(&shifted_notes);

    // Enable loop region [0, 1500) frames (1/16 beat at 120 BPM / 48 kHz) and verify 3 loop passes match oracle.
    for editor in [&mut source_editor, &mut oracle_editor] {
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 1_500,
                enabled: true,
            })
            .unwrap();
    }

    // Also add a Session MIDI clip on scene 0 so we can test Arrangement <-> Session transitions.
    let Created::Scene(scene) = source_editor
        .execute(Command::AddScene {
            name: "scene 1".into(),
        })
        .unwrap()
    else {
        panic!("expected scene");
    };
    let Created::SessionClip(session_clip) = source_editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session-midi".into(),
            source_length: beat_frac(1, 16),
        })
        .unwrap()
    else {
        panic!("expected session clip");
    };
    source_editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 32),
            pitch: 62,
            channel: 0,
            velocity: 100,
        })
        .unwrap();

    let source_rev = prepare_revision(source_editor.project());
    let oracle_rev = prepare_revision(oracle_editor.project());
    let transpose =
        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse).unwrap();

    let loop_frames = 4_500_usize; // 3 full loop passes
    let mut oracle = SessionPlayback::new(Arc::clone(&oracle_rev)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_loop = vec![[0.0_f32; 2]; loop_frames];
    oracle.render(&mut expected_loop).unwrap();

    let mut playback = SessionPlayback::new(Arc::clone(&source_rev)).unwrap();
    playback
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose)
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_loop = vec![[0.0_f32; 2]; loop_frames];
    for chunk in actual_loop.chunks_mut(128) {
        playback.render(chunk).unwrap();
    }
    assert_eq!(actual_loop, expected_loop);

    // Stop mid-note -> immediate quiescence -> Seek(0) -> Play reproduces bit-identically.
    let mut mid_buf = [[0.0_f32; 2]; 256];
    playback.render(&mut mid_buf).unwrap();
    assert!(
        !playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut replayed = vec![[0.0_f32; 2]; loop_frames];
    playback.render(&mut replayed).unwrap();
    assert_eq!(replayed, expected_loop);

    // Route transition: launch Session clip mid-note, render, StopTrack, drain tail -> zero stuck notes.
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut route_buf = vec![[0.0_f32; 2]; 1_024];
    playback.render(&mut route_buf).unwrap();
    assert!(route_buf.iter().flatten().any(|s| s.abs() > 1e-3));

    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut drain = vec![[0.0_f32; 2]; 1_024];
    playback.render(&mut drain).unwrap();
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert!(
        playback
            .tail_mutable_instruments
            .iter()
            .flatten()
            .all(|tail| tail.committed_is_quiescent())
    );

    // BackToArrangement -> Play -> Stop leaves zero stuck notes.
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.render(&mut mid_buf).unwrap();
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
}

#[test]
fn revision_handoff_changed_processor_stale_publication_and_refusal_preserve_state() {
    let notes = [(BeatTime::ZERO, beat_frac(1, 32), 60, 110)];
    let (editor, track) = build_subtractive_project(&notes);
    let rev = prepare_revision(editor.project());

    let transpose_7 =
        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse).unwrap();
    let transpose_12 =
        PreparedTransposeMidiProcessor::new(12, TransposeOutOfRangePolicy::Refuse).unwrap();

    // 1. Continuous reference with +7 transpose.
    let mut continuous = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    continuous
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_7)
        .unwrap();
    continuous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = vec![[0.0_f32; 2]; 2_000];
    continuous.render(&mut expected).unwrap();

    // 2. Compatible revision handoff mid-note inherits processor + instrument state seamlessly.
    let mut active = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    active
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_7)
        .unwrap();
    active.apply(SessionPlaybackCommand::Play).unwrap();
    let mut handed_off = vec![[0.0_f32; 2]; 2_000];
    active.render(&mut handed_off[..300]).unwrap();
    assert!(
        !active.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Simulate a stale candidate created and dropped on control without handoff: active is untouched.
    {
        let mut stale_candidate = SessionPlayback::new(Arc::clone(&rev)).unwrap();
        stale_candidate
            .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_12)
            .unwrap();
        drop(stale_candidate);
    }

    let mut compatible_candidate = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    compatible_candidate
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_7)
        .unwrap();
    compatible_candidate
        .try_inherit_state_from(&mut active)
        .unwrap();
    assert!(
        compatible_candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    compatible_candidate.render(&mut handed_off[300..]).unwrap();
    assert_eq!(handed_off, expected);
    assert!(
        compatible_candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 3. Changing MIDI processor (+7 -> +12) while note is held refuses cleanly without corrupting active +7 playback;
    // once +7 finishes to quiescence, the +12 candidate hands off cleanly.
    let mut playing_7 = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    playing_7
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_7)
        .unwrap();
    playing_7.apply(SessionPlaybackCommand::Play).unwrap();
    let mut active_full = vec![[0.0_f32; 2]; 2_000];
    playing_7.render(&mut active_full[..300]).unwrap();

    let mut changed_candidate = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    changed_candidate
        .install_track_midi_processor_on_control(track, midi_proc_id(), &transpose_12)
        .unwrap();
    assert_eq!(
        changed_candidate.try_inherit_state_from(&mut playing_7),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    // Refused handoff leaves active +7 playback untouched and bit-identical through release tail.
    playing_7.render(&mut active_full[300..]).unwrap();
    assert_eq!(active_full, expected);
    assert!(
        playing_7.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    // Once quiescent, the +12 candidate inherits transport state cleanly.
    changed_candidate
        .try_inherit_state_from(&mut playing_7)
        .unwrap();
    assert!(
        changed_candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 4. Out-of-range transpose refusal preserves committed state and leaves no stuck notes.
    let high_notes = [(BeatTime::ZERO, beat_frac(1, 32), 122, 100)];
    let (high_editor, high_track) = build_subtractive_project(&high_notes);
    let high_rev = prepare_revision(high_editor.project());
    let mut refusing = SessionPlayback::new(high_rev).unwrap();
    refusing
        .install_track_midi_processor_on_control(high_track, midi_proc_id(), &transpose_12)
        .unwrap();
    refusing.apply(SessionPlaybackCommand::Play).unwrap();
    let mut err_buf = [[0.0_f32; 2]; 64];
    let err = refusing.render(&mut err_buf).unwrap_err();
    assert!(matches!(
        err,
        SessionPlaybackError::MutableInstrumentDiagnostic(_)
    ));
    assert!(
        refusing.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
}

#[test]
fn canonical_pitch_device_renders_exact_shifted_pcm_and_handles_mid_note_changes_and_bypass() {
    use hex_devices::midi_pitch::{PITCH_TYPE_ID, PitchOverflowPolicy, PitchSettings};

    // Note 1: C4 (60) from 0..1/32 (0..750 frames);
    // Note 2: C4 (60) overlapping from 1/64..3/64 (375..1125 frames);
    // Note 3: C4 (60) starting after handoff at 1/16..3/32 (1500..2250 frames).
    let authored_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 90),
        (beat_frac(1, 16), beat_frac(1, 32), 60, 105),
    ];

    // 1. Canonical project with Pitch (+7 st, Clamp) matches shifted PCM oracle bit-for-bit.
    let (mut editor, track) = build_subtractive_project(&authored_notes);
    let pitch_7 = PitchSettings {
        semitones: 7,
        ..PitchSettings::default()
    }
    .encode()
    .unwrap();
    let Created::Device(pitch_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: pitch_7.clone(),
        })
        .unwrap()
    else {
        panic!("expected midi processor id");
    };
    let (oracle_editor, _) = build_subtractive_project(&[
        (BeatTime::ZERO, beat_frac(1, 32), 67, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 67, 90),
        (beat_frac(1, 16), beat_frac(1, 32), 67, 105),
    ]);

    let rev_7 = prepare_revision(editor.project());
    let oracle_rev = prepare_revision(oracle_editor.project());
    let total_frames = 3_000_usize;

    let mut oracle = SessionPlayback::new(oracle_rev).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_7 = vec![[0.0_f32; 2]; total_frames];
    oracle.render(&mut expected_7).unwrap();

    let mut playback_7 = SessionPlayback::new(Arc::clone(&rev_7)).unwrap();
    playback_7.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_7 = vec![[0.0_f32; 2]; total_frames];
    for chunk in actual_7.chunks_mut(128) {
        playback_7.render(chunk).unwrap();
    }
    assert_eq!(actual_7, expected_7);
    assert!(
        playback_7.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 2. Transposition change (+7 -> +12) WHILE notes 1 and 2 are held!
    // Notes 1 & 2 stay at +7 (67) and release cleanly; Note 3 starts at +12 (72)!
    let pitch_12 = PitchSettings {
        semitones: 12,
        ..PitchSettings::default()
    }
    .encode()
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: pitch_id,
            state: pitch_12,
        })
        .unwrap();
    let rev_12 = prepare_revision(editor.project());

    let (mixed_oracle_editor, _) = build_subtractive_project(&[
        (BeatTime::ZERO, beat_frac(1, 32), 67, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 67, 90),
        (beat_frac(1, 16), beat_frac(1, 32), 72, 105),
    ]);
    let mut mixed_oracle =
        SessionPlayback::new(prepare_revision(mixed_oracle_editor.project())).unwrap();
    mixed_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_mixed = vec![[0.0_f32; 2]; total_frames];
    mixed_oracle.render(&mut expected_mixed).unwrap();

    let mut live_a = SessionPlayback::new(Arc::clone(&rev_7)).unwrap();
    live_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_mixed = vec![[0.0_f32; 2]; total_frames];
    // Render 500 frames: both overlapping C4 notes are active!
    live_a.render(&mut actual_mixed[..500]).unwrap();
    assert!(
        !live_a.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    let mut live_b = SessionPlayback::new(Arc::clone(&rev_12)).unwrap();
    live_b.try_inherit_state_from(&mut live_a).unwrap();
    live_b.render(&mut actual_mixed[500..]).unwrap();
    assert_eq!(actual_mixed, expected_mixed);
    assert!(
        live_b.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 3. Bypass toggled ON mid-note: already-held +7 notes release cleanly; Note 3 plays untransposed (60)!
    editor.undo(); // back to +7
    editor
        .execute(Command::SetDeviceBypass {
            device: pitch_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = prepare_revision(editor.project());

    let (bypass_oracle_editor, _) = build_subtractive_project(&[
        (BeatTime::ZERO, beat_frac(1, 32), 67, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 67, 90),
        (beat_frac(1, 16), beat_frac(1, 32), 60, 105),
    ]);
    let mut bypass_oracle =
        SessionPlayback::new(prepare_revision(bypass_oracle_editor.project())).unwrap();
    bypass_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_bypass = vec![[0.0_f32; 2]; total_frames];
    bypass_oracle.render(&mut expected_bypass).unwrap();

    let mut active_pre_bypass = SessionPlayback::new(Arc::clone(&rev_7)).unwrap();
    active_pre_bypass
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut actual_bypass = vec![[0.0_f32; 2]; total_frames];
    active_pre_bypass.render(&mut actual_bypass[..500]).unwrap();
    let mut bypassed_cand = SessionPlayback::new(Arc::clone(&rev_bypassed)).unwrap();
    bypassed_cand
        .try_inherit_state_from(&mut active_pre_bypass)
        .unwrap();
    bypassed_cand.render(&mut actual_bypass[500..]).unwrap();
    assert_eq!(actual_bypass, expected_bypass);
    assert!(
        bypassed_cand.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 4. Drop policy at upper MIDI boundary suppresses note cleanly and produces pure silence.
    let pitch_drop = PitchSettings {
        semitones: 24,
        overflow: PitchOverflowPolicy::Drop,
        ..PitchSettings::default()
    }
    .encode()
    .unwrap();
    let (mut drop_editor, drop_track) =
        build_subtractive_project(&[(BeatTime::ZERO, beat_frac(1, 32), 115, 110)]);
    drop_editor
        .execute(Command::InsertMidiProcessor {
            track: drop_track,
            state: pitch_drop,
        })
        .unwrap();
    let mut drop_pb = SessionPlayback::new(prepare_revision(drop_editor.project())).unwrap();
    drop_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut silent = vec![[0.0_f32; 2]; 2_000];
    drop_pb.render(&mut silent).unwrap();
    assert!(silent.iter().flatten().all(|s| *s == 0.0));
    assert!(
        drop_pb.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // 5. Removal + Stop leave zero stuck notes, and malformed state refuses at preparation.
    editor
        .execute(Command::RemoveMidiProcessor { track })
        .unwrap();
    let rev_removed = prepare_revision(editor.project());
    let mut removed_pb = SessionPlayback::new(rev_removed).unwrap();
    removed_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut mid = [[0.0_f32; 2]; 256];
    removed_pb.render(&mut mid).unwrap();
    removed_pb.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(
        removed_pb.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    let malformed =
        hex_lattice_contract::NativeDeviceState::new(PITCH_TYPE_ID, 1, vec![0, 0]).unwrap();
    editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: malformed,
        })
        .unwrap();
    assert!(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .is_err()
    );
}

#[test]
fn canonical_chord_device_matches_pcm_oracle_across_overlaps_live_edits_bypass_and_loop() {
    use hex_devices::midi_chord::ChordSettings;

    // Two overlapping C4 notes + a 3rd C4 after live edit / bypass handoff.
    let authored_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 100),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 80),
        (beat_frac(1, 16), beat_frac(1, 32), 60, 100),
    ];

    let major = ChordSettings {
        shifts: [4, 7, 0, 0, 0, 0],
        ..ChordSettings::default()
    };
    let (mut editor, track) = build_subtractive_project(&authored_notes);
    let Created::Device(chord_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: major.encode().unwrap(),
        })
        .unwrap()
    else {
        panic!("expected chord device id");
    };

    // Exact multi-note PCM oracle for major triad (root, +4, +7) on every authored note.
    let mut oracle_notes = Vec::new();
    for &(start, dur, pitch, vel) in &authored_notes {
        oracle_notes.push((start, dur, pitch, vel));
        oracle_notes.push((start, dur, pitch + 4, vel));
        oracle_notes.push((start, dur, pitch + 7, vel));
    }
    let (oracle_editor, _) = build_subtractive_project(&oracle_notes);

    let rev_major = prepare_revision(editor.project());
    let oracle_rev = prepare_revision(oracle_editor.project());
    let total_frames = 3_000_usize;

    let mut oracle = SessionPlayback::new(oracle_rev).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_major = vec![[0.0_f32; 2]; total_frames];
    oracle.render(&mut expected_major).unwrap();
    assert!(expected_major.iter().flatten().any(|s| s.abs() > 1e-3));

    // Chunked render matches authored multi-note oracle bit-for-bit!
    let mut pb_major = SessionPlayback::new(Arc::clone(&rev_major)).unwrap();
    pb_major.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_major = vec![[0.0_f32; 2]; total_frames];
    for chunk in actual_major.chunks_mut(128) {
        pb_major.render(chunk).unwrap();
    }
    assert_eq!(actual_major, expected_major);
    assert!(
        pb_major.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Live voicing change (major -> minor: +3, +7) WHILE notes 1 & 2 are held!
    // Notes 1 & 2 release all 6 major voices cleanly; Note 3 plays minor triad (60, 63, 67).
    let minor = ChordSettings {
        shifts: [3, 7, 0, 0, 0, 0],
        ..ChordSettings::default()
    };
    editor
        .execute(Command::ReplaceDeviceState {
            device: chord_id,
            state: minor.encode().unwrap(),
        })
        .unwrap();
    let rev_minor = prepare_revision(editor.project());

    let mixed_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 100),
        (BeatTime::ZERO, beat_frac(1, 32), 64, 100),
        (BeatTime::ZERO, beat_frac(1, 32), 67, 100),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 80),
        (beat_frac(1, 64), beat_frac(1, 32), 64, 80),
        (beat_frac(1, 64), beat_frac(1, 32), 67, 80),
        (beat_frac(1, 16), beat_frac(1, 32), 60, 100),
        (beat_frac(1, 16), beat_frac(1, 32), 63, 100),
        (beat_frac(1, 16), beat_frac(1, 32), 67, 100),
    ];
    let (mixed_oracle_editor, _) = build_subtractive_project(&mixed_notes);
    let mut mixed_oracle =
        SessionPlayback::new(prepare_revision(mixed_oracle_editor.project())).unwrap();
    mixed_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_mixed = vec![[0.0_f32; 2]; total_frames];
    mixed_oracle.render(&mut expected_mixed).unwrap();

    let mut live_a = SessionPlayback::new(Arc::clone(&rev_major)).unwrap();
    live_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_mixed = vec![[0.0_f32; 2]; total_frames];
    live_a.render(&mut actual_mixed[..500]).unwrap();
    let mut live_b = SessionPlayback::new(Arc::clone(&rev_minor)).unwrap();
    live_b.try_inherit_state_from(&mut live_a).unwrap();
    live_b.render(&mut actual_mixed[500..]).unwrap();
    assert_eq!(actual_mixed, expected_mixed);
    assert!(
        live_b.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Mid-note bypass toggle: held major chord releases all 6 voices; Note 3 plays dry root (60)!
    editor.undo(); // back to major
    editor
        .execute(Command::SetDeviceBypass {
            device: chord_id,
            bypassed: true,
        })
        .unwrap();
    let rev_bypassed = prepare_revision(editor.project());

    let bypass_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 100),
        (BeatTime::ZERO, beat_frac(1, 32), 64, 100),
        (BeatTime::ZERO, beat_frac(1, 32), 67, 100),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 80),
        (beat_frac(1, 64), beat_frac(1, 32), 64, 80),
        (beat_frac(1, 64), beat_frac(1, 32), 67, 80),
        (beat_frac(1, 16), beat_frac(1, 32), 60, 100),
    ];
    let (bypass_oracle_editor, _) = build_subtractive_project(&bypass_notes);
    let mut bypass_oracle =
        SessionPlayback::new(prepare_revision(bypass_oracle_editor.project())).unwrap();
    bypass_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_bypass = vec![[0.0_f32; 2]; total_frames];
    bypass_oracle.render(&mut expected_bypass).unwrap();

    let mut pre_bypass = SessionPlayback::new(Arc::clone(&rev_major)).unwrap();
    pre_bypass.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_bypass = vec![[0.0_f32; 2]; total_frames];
    pre_bypass.render(&mut actual_bypass[..500]).unwrap();
    let mut bypassed = SessionPlayback::new(rev_bypassed).unwrap();
    bypassed.try_inherit_state_from(&mut pre_bypass).unwrap();
    bypassed.render(&mut actual_bypass[500..]).unwrap();
    assert_eq!(actual_bypass, expected_bypass);
    assert!(
        bypassed.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Stop mid-chord & Seek(0) leave zero stuck notes.
    let mut stop_pb = SessionPlayback::new(Arc::clone(&rev_major)).unwrap();
    stop_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut mid = [[0.0_f32; 2]; 256];
    stop_pb.render(&mut mid).unwrap();
    stop_pb.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(
        stop_pb.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    stop_pb.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    stop_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut replayed = vec![[0.0_f32; 2]; total_frames];
    stop_pb.render(&mut replayed).unwrap();
    assert_eq!(replayed, expected_major);
}

#[test]
fn canonical_velocity_device_matches_pcm_oracle_across_fixed_gate_live_edits_and_bypass() {
    use hex_devices::midi_velocity::{VelocityMode, VelocitySettings};

    let authored_notes = [
        (BeatTime::ZERO, beat_frac(1, 32), 60, 40),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 100),
        (beat_frac(1, 16), beat_frac(1, 32), 64, 30),
    ];

    // Fixed(110) maps every note velocity to 110 -> matches authored vel=110 PCM oracle bit-for-bit.
    let fixed_110 = VelocitySettings {
        mode: VelocityMode::Fixed,
        out_high: 110,
        ..VelocitySettings::default()
    };
    let (mut editor, track) = build_subtractive_project(&authored_notes);
    let Created::Device(vel_id) = editor
        .execute(Command::InsertMidiProcessor {
            track,
            state: fixed_110.encode().unwrap(),
        })
        .unwrap()
    else {
        panic!("expected velocity processor id");
    };
    let (oracle_editor, _) = build_subtractive_project(&[
        (BeatTime::ZERO, beat_frac(1, 32), 60, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 110),
        (beat_frac(1, 16), beat_frac(1, 32), 64, 110),
    ]);

    let rev_fixed = prepare_revision(editor.project());
    let total_frames = 3_000_usize;

    let mut oracle = SessionPlayback::new(prepare_revision(oracle_editor.project())).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_fixed = vec![[0.0_f32; 2]; total_frames];
    oracle.render(&mut expected_fixed).unwrap();

    let mut pb_fixed = SessionPlayback::new(Arc::clone(&rev_fixed)).unwrap();
    pb_fixed.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_fixed = vec![[0.0_f32; 2]; total_frames];
    for chunk in actual_fixed.chunks_mut(128) {
        pb_fixed.render(chunk).unwrap();
    }
    assert_eq!(actual_fixed, expected_fixed);
    assert!(
        pb_fixed.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    // Live edit to Gate([60..=127]) mid-note: already-held notes 1 & 2 release cleanly; Note 3 (vel=30) is gated!
    let gate_60 = VelocitySettings {
        mode: VelocityMode::Gate,
        lowest: 60,
        range: 67,
        ..VelocitySettings::default()
    };
    editor
        .execute(Command::ReplaceDeviceState {
            device: vel_id,
            state: gate_60.encode().unwrap(),
        })
        .unwrap();
    let rev_gate = prepare_revision(editor.project());

    let (mixed_oracle_editor, _) = build_subtractive_project(&[
        (BeatTime::ZERO, beat_frac(1, 32), 60, 110),
        (beat_frac(1, 64), beat_frac(1, 32), 60, 110),
    ]);
    let mut mixed_oracle =
        SessionPlayback::new(prepare_revision(mixed_oracle_editor.project())).unwrap();
    mixed_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected_mixed = vec![[0.0_f32; 2]; total_frames];
    mixed_oracle.render(&mut expected_mixed).unwrap();

    let mut live_a = SessionPlayback::new(Arc::clone(&rev_fixed)).unwrap();
    live_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual_mixed = vec![[0.0_f32; 2]; total_frames];
    live_a.render(&mut actual_mixed[..500]).unwrap();
    let mut live_b = SessionPlayback::new(Arc::clone(&rev_gate)).unwrap();
    live_b.try_inherit_state_from(&mut live_a).unwrap();
    live_b.render(&mut actual_mixed[500..]).unwrap();
    assert_eq!(actual_mixed, expected_mixed);
    assert!(
        live_b.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
}
