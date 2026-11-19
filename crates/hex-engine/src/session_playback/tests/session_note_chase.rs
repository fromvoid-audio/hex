//! Public transport actions exercise policy custody; no private policy injection.
use super::*;
const Q: u64 = 1 << 32;

fn fixture(oracle: bool) -> Project {
    let mut snapshot = session_only_project(&project(1.0, 0, Q)).snapshot();
    snapshot.sample_rate = 8_000;
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("midi fixture");
    };
    clip.playback_offset_raw = if oracle { 0 } else { 3 * Q / 16 };
    clip.source = MidiSourceSnapshot {
        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
        source_length_raw: Q / 4,
        source_start_raw: if oracle { 3 * Q / 16 } else { 0 },
        loop_start_raw: Q / 16,
        loop_end_raw: Q / 4,
        looping: true,
        notes: vec![
            MidiNoteSnapshot {
                active: true,
                id: 1_000_001,
                start_raw: Q / 8,
                duration_raw: Q / 8,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                active: true,
                id: 1_000_002,
                start_raw: 3 * Q / 16,
                duration_raw: Q / 64,
                pitch: 64,
                channel: 0,
                velocity: 100,
            },
        ],
    };
    snapshot.next_id = 1_000_003;
    legacy_project_from_snapshot(snapshot).unwrap()
}
fn prepared(oracle: bool, mutable: bool) -> Arc<PreparedRevision> {
    let project = fixture(oracle);
    if mutable {
        let definition = super::source_loop_tests::mutable_definition();
        let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
        revision_with_instrument_registry(
            &with_test_instrument(&project, definition.metadata.id, 0),
            Arc::new(hex_devices::SineTable::new()),
            &registry,
        )
    } else {
        revision(&project, Arc::new(hex_devices::SineTable::new()))
    }
}
fn start(revision: Arc<PreparedRevision>) -> Playback {
    let clip = revision.session().midi_clips()[0].id;
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(
            PlaybackCommand::PlayFromWithNoteChase {
                frame: 0,
                note_chase: crate::NoteChase::Off,
            },
        ))
        .unwrap();
    playback
}
fn render(playback: &mut Playback, frames: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for block in output.chunks_mut(17) {
        playback.render(block).unwrap();
    }
    output
}

#[test]
fn selected_session_off_keeps_boundary_release_and_later_repeats_through_handoff_and_pause() {
    for mutable in [false, true] {
        let actual_revision = prepared(false, mutable);
        let expected_revision = prepared(true, mutable);
        let mut actual = start(Arc::clone(&actual_revision));
        let mut expected = start(Arc::clone(&expected_revision));
        let first = render(&mut expected, 16);
        assert!(
            first.iter().flatten().any(|s| *s != 0.0),
            "real boundary onset remains admitted"
        );
        assert_eq!(render(&mut actual, 16), first);
        let mut next =
            Playback::try_from_prepared_with_session(Arc::clone(&actual_revision)).unwrap();
        next.try_inherit_runtime_from(&mut actual).unwrap();
        actual = next;
        for playback in [&mut actual, &mut expected] {
            playback
                .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Pause))
                .unwrap();
        }
        let release = render(&mut expected, 32);
        assert!(
            release.iter().flatten().any(|s| *s != 0.0),
            "admitted voice keeps its release"
        );
        assert_eq!(
            render(&mut actual, 32),
            release,
            "no clipped gate may be reconstructed"
        );
        for playback in [&mut actual, &mut expected] {
            playback
                .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
                .unwrap();
        }
        let later = render(&mut expected, 1_100);
        assert!(
            later[500..].iter().flatten().any(|s| *s != 0.0),
            "later source repeat remains eligible"
        );
        assert_eq!(render(&mut actual, 1_100), later);
    }
}

#[test]
fn raw_relaunch_seek_stop_and_cue_restore_default_on_without_a_sticky_preference() {
    let revision = prepared(false, false);
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    for reset in 0..4 {
        let mut actual = start(Arc::clone(&revision));
        let mut expected = start(Arc::clone(&revision));
        expected
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(0)))
            .unwrap();
        match reset {
            0 => actual
                .try_apply_action(PlaybackAction::Session(
                    SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    },
                ))
                .unwrap(),
            1 => actual
                .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(0)))
                .unwrap(),
            2 => {
                actual
                    .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Stop))
                    .unwrap();
                actual
                    .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
                    .unwrap();
            }
            _ => actual
                .try_apply_action(PlaybackAction::Session(
                    SessionPlaybackCommand::CueMidiClip {
                        track,
                        clip,
                        source_beat: BeatTime::from_raw(3 * Q / 16),
                        expected_route_generation: actual.session_route_state_generation(),
                    },
                ))
                .unwrap(),
        }
        assert_eq!(
            render(&mut actual, 32),
            render(&mut expected, 32),
            "reset={reset}"
        );
    }
}

#[test]
fn queued_raw_launch_restores_on_after_the_active_off_incarnation() {
    let revision = prepared(false, false);
    let clip = revision.session().midi_clips()[0].id;
    let mut actual = start(Arc::clone(&revision));
    render(&mut actual, 16);
    actual
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::ThirtySecond,
            },
        ))
        .unwrap();
    // At 8 kHz/120 bpm the next 1/32 grid line is frame 500.
    let before = render(&mut actual, 484);
    let mut oracle = start(Arc::clone(&revision));
    render(&mut oracle, 16);
    assert_eq!(
        before,
        render(&mut oracle, 484),
        "queued request does not alter the active policy"
    );
    oracle
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    assert_eq!(
        render(&mut actual, 64),
        render(&mut oracle, 64),
        "pending raw launch captures fresh On"
    );
}
