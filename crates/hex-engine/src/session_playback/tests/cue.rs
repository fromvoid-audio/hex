use super::*;

const Q: u64 = 1_u64 << 32;
fn beat(value: u64) -> BeatTime {
    BeatTime::from_raw(value * Q)
}
fn cue_project() -> Project {
    let mut snapshot = session_only_project(&project(1.0, 0, 12 * Q)).snapshot();
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("midi source")
    };
    clip.source.notes.extend([
        MidiNoteSnapshot {
            active: true,
            id: 1_000_008,
            start_raw: 10 * Q,
            duration_raw: Q,
            pitch: 64,
            channel: 0,
            velocity: 93,
        },
        MidiNoteSnapshot {
            active: true,
            id: 1_000_009,
            start_raw: 11 * Q,
            duration_raw: Q / 2,
            pitch: 67,
            channel: 0,
            velocity: 37,
        },
    ]);
    snapshot.next_id = 1_000_010;
    legacy_project_from_snapshot(snapshot).unwrap()
}
fn runtime(project: &Project) -> SessionPlayback {
    SessionPlayback::new(revision(project, Arc::new(hex_devices::SineTable::new()))).unwrap()
}
fn launch(runtime: &mut SessionPlayback) -> (TrackId, ClipId) {
    let prepared = &runtime.revision.session().midi_clips()[0];
    let (track, clip) = (prepared.track, prepared.id);
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    (track, clip)
}
fn cue(
    runtime: &mut SessionPlayback,
    track: TrackId,
    clip: ClipId,
    source_beat: BeatTime,
) -> Result<(), SessionPlaybackError> {
    runtime.apply(SessionPlaybackCommand::CueMidiClip {
        track,
        clip,
        source_beat,
        expected_route_generation: runtime.route_state_generation(),
    })
}

#[test]
fn cue_captures_old_origin_release_then_matches_literal_start_without_chasing_and_keeps_clocks() {
    let project = cue_project();
    let saved = project.snapshot();
    let mut actual = runtime(&project);
    let (track, clip) = launch(&mut actual);
    actual.apply(SessionPlaybackCommand::Play).unwrap();
    actual.render(&mut [[0.0; 2]; 1000]).unwrap();
    actual
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert!(actual.route_status(0).unwrap().pending.is_some());
    let before = (
        actual.arrangement_frame(),
        actual.performance_frame(),
        actual.playing(),
    );
    cue(&mut actual, track, clip, beat(10)).unwrap();
    assert_eq!(
        (
            actual.arrangement_frame(),
            actual.performance_frame(),
            actual.playing()
        ),
        before
    );
    let status = actual.route_status(0).unwrap();
    assert!(status.pending.is_none());
    let origin = status.active_origin.unwrap();
    assert_eq!(origin.elapsed_at_anchor, beat(10));
    assert_eq!(
        actual
            .timing_view()
            .beat_to_frame(origin.performance_anchor)
            .unwrap(),
        before.1
    );
    assert!(
        !actual.releases.is_empty(),
        "outgoing sounding gate has bounded old-origin release custody"
    );
    let release_end = actual
        .releases
        .iter()
        .map(|entry| entry.voice.end_frame())
        .max()
        .unwrap();

    let mut literal = saved.clone();
    let SessionCellContentSnapshot::Midi(literal_clip) = &mut literal.session.cells[0].content
    else {
        panic!("source")
    };
    literal_clip.source.source_start_raw = 10 * Q;
    let literal = legacy_project_from_snapshot(literal).unwrap();
    let mut expected = runtime(&literal);
    expected
        .apply(SessionPlaybackCommand::Seek(before.1))
        .unwrap();
    launch(&mut expected);
    expected.apply(SessionPlaybackCommand::Play).unwrap();
    let width = usize::try_from(release_end - before.1 + 513).unwrap();
    let mut output = vec![[0.0; 2]; width];
    let mut oracle = vec![[0.0; 2]; width];
    actual.render(&mut output).unwrap();
    expected.render(&mut oracle).unwrap();
    let tail = usize::try_from(release_end - before.1).unwrap();
    assert_eq!(
        output[tail..],
        oracle[tail..],
        "after finite old tail, cue is the literal no-chase source start"
    );
    assert!(
        output[tail..]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-5)
    );
    assert_eq!(
        project.snapshot(),
        saved,
        "transient cue never edits saved source markers or notes"
    );
}

#[test]
fn paused_cue_preserves_pause_and_stop_seek_keep_their_existing_zero_elapsed_reset() {
    let project = cue_project();
    let mut playback = runtime(&project);
    let (track, clip) = launch(&mut playback);
    cue(&mut playback, track, clip, beat(10)).unwrap();
    assert!(!playback.playing());
    assert_eq!(
        (playback.arrangement_frame(), playback.performance_frame()),
        (0, 0)
    );
    assert_eq!(
        playback.route_status(0).unwrap().active_origin,
        Some(SessionMidiOrigin::new(BeatTime::ZERO, beat(10)))
    );
    playback.render(&mut [[0.0; 2]; 8]).unwrap();
    assert_eq!(playback.performance_frame(), 0);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut audible = [[0.0; 2]; 64];
    playback.render(&mut audible).unwrap();
    assert!(audible.iter().flatten().any(|sample| sample.abs() > 1e-5));
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(
        playback.route_status(0).unwrap().active_origin,
        Some(SessionMidiOrigin::launch(BeatTime::ZERO))
    );
    cue(&mut playback, track, clip, beat(10)).unwrap();
    playback.apply(SessionPlaybackCommand::Seek(24000)).unwrap();
    assert_eq!(
        playback.route_status(0).unwrap().active_origin,
        Some(SessionMidiOrigin::launch(beat(1)))
    );
}

#[test]
fn stale_invalid_inactive_and_release_capacity_cues_refuse_without_audible_state_commit() {
    let project = cue_project();
    let mut playback = runtime(&project);
    let clip = playback.revision.session().midi_clips()[0].id;
    let track = project.tracks()[0].id();
    assert_eq!(
        cue(&mut playback, track, clip, beat(10)),
        Err(SessionPlaybackError::InvalidCommand)
    );
    launch(&mut playback);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback.render(&mut [[0.0; 2]; 1000]).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let status = playback.route_status(0).unwrap();
    let generation = playback.route_state_generation();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::CueMidiClip {
            track,
            clip,
            source_beat: beat(10),
            expected_route_generation: generation.wrapping_sub(1)
        }),
        Err(SessionPlaybackError::InvalidCommand)
    );
    assert!(
        cue(&mut playback, track, clip, beat(32)).is_err(),
        "exclusive source end refuses"
    );
    assert_eq!(playback.route_status(0).unwrap(), status);
    assert_eq!(playback.route_state_generation(), generation);
    let mut reference = runtime(&project);
    launch(&mut reference);
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut [[0.0; 2]; 1000]).unwrap();
    let saved_capacity = playback.release_capacity;
    playback.release_capacity = 0;
    assert_eq!(
        cue(&mut playback, track, clip, beat(10)),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    playback.release_capacity = saved_capacity;
    assert_eq!(playback.route_status(0).unwrap(), status);
    assert_eq!(playback.route_state_generation(), generation);
    assert!(playback.releases.is_empty());
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    playback.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "refusal does not cut outgoing DSP or mutate canonical route/pending"
    );
}

#[test]
fn cue_after_tempo_handoff_uses_effective_clock_and_observation_anchor_then_survives_another_handoff()
 {
    let project = cue_project();
    let mut old = runtime(&project);
    let mut oracle_old = runtime(&project);
    let (track, clip) = launch(&mut old);
    assert_eq!(launch(&mut oracle_old), (track, clip));
    for runtime in [&mut old, &mut oracle_old] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 1000]).unwrap();
    }
    let mut editor = Editor::new(project, 16).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let mut current = runtime(editor.project());
    current.try_inherit_state_from(&mut old).unwrap();
    let mut oracle = runtime(editor.project());
    oracle.try_inherit_state_from(&mut oracle_old).unwrap();
    cue(&mut oracle, track, clip, beat(10)).unwrap();
    assert!(current.clock_anchor().is_some());
    cue(&mut current, track, clip, beat(10)).unwrap();
    let status = current.route_status(0).unwrap();
    let origin = status.active_origin.unwrap();
    assert_eq!(
        current
            .timing_view()
            .beat_to_frame(origin.performance_anchor)
            .unwrap(),
        1000
    );
    assert_ne!(
        current
            .revision
            .session()
            .timing()
            .beat_to_frame(origin.performance_anchor)
            .unwrap(),
        1000,
        "base map would misplace this cue"
    );
    let observation =
        crate::PreparedPlaybackObservation::prepare_on_control(&current.revision).unwrap();
    assert_eq!(
        current
            .revision
            .midi_performance_beat_at(1000, current.clock_anchor())
            .unwrap(),
        origin.performance_anchor
    );
    assert_eq!(
        observation
            .activity()
            .performance_beat_at(1000, current.clock_anchor())
            .unwrap(),
        origin.performance_anchor
    );
    let mut spans = MidiSpanBuffer::default();
    for keys in [
        current
            .revision
            .midi_gate_keys(
                track,
                status.owner,
                status.active_origin,
                current.clock_anchor(),
                (1000, 1000),
                &mut spans,
            )
            .unwrap(),
        observation
            .activity()
            .gate_keys(
                track,
                status.owner,
                status.active_origin,
                current.clock_anchor(),
                (1000, 1000),
                &mut spans,
            )
            .unwrap(),
    ] {
        assert_eq!(keys, [0, 1], "exact cued pitch64, no historical pitch60");
    }
    current.render(&mut [[0.0; 2]; 64]).unwrap();
    oracle.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut next = runtime(editor.project());
    next.try_inherit_state_from(&mut current).unwrap();
    assert_eq!(next.route_status(0).unwrap().active_origin, Some(origin));
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    next.render(&mut actual).unwrap();
    // Handoff moves captured suffix custody from current; compare against an
    // independent owner rather than attempting to render the retired donor.
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "later graph handoff retains cued source phase and effective global beat"
    );
}

#[test]
fn cue_stream_plan_is_targeted_route_custody_not_seek_or_clock_mutation() {
    let project = cue_project();
    let mut playback = runtime(&project);
    let (track, clip) = launch(&mut playback);
    let before = playback.route_status(0).unwrap();
    let prepared = playback
        .prepare_stream_action(PlaybackAction::Session(
            SessionPlaybackCommand::CueMidiClip {
                track,
                clip,
                source_beat: beat(10),
                expected_route_generation: playback.route_state_generation(),
            },
        ))
        .unwrap();
    assert_eq!(prepared.scheduled_due_frame(), None);
    assert!(!prepared.playing_after_commit());
    assert!(
        playback
            .stream_action_targets_track(&prepared, Some(track))
            .unwrap()
    );
    assert_eq!(
        playback.route_status(0).unwrap(),
        before,
        "preparation cannot commit cue or erase pending state"
    );
    assert_eq!(
        (
            playback.performance_frame(),
            playback.arrangement_frame(),
            playback.playing()
        ),
        (0, 0, false)
    );
    let PlaybackAction::Session(command) = prepared.action() else {
        panic!("route action")
    };
    playback.apply(command).unwrap();
    assert_eq!(
        playback
            .route_status(0)
            .unwrap()
            .active_origin
            .unwrap()
            .elapsed_at_anchor,
        beat(10)
    );
}
