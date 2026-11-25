//! Public transport actions exercise policy custody; no private policy injection.
use super::*;
const Q: u64 = 1 << 32;

pub(crate) fn fixture(oracle: bool) -> Project {
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
                expression: Default::default(),
                expression_points: Vec::new(),
            },
            MidiNoteSnapshot {
                active: true,
                id: 1_000_002,
                start_raw: 3 * Q / 16,
                duration_raw: Q / 64,
                pitch: 64,
                channel: 0,
                velocity: 100,
                expression: Default::default(),
                expression_points: Vec::new(),
            },
        ],
        expression_lanes: Vec::new(),
    };
    snapshot.next_id = 1_000_003;
    legacy_project_from_snapshot(snapshot).unwrap()
}
/// Apply the same canonical source window to an existing device fixture.
/// Device identity/state and all construction remain outside measured renders.
pub(super) fn with_window(project: &Project, oracle: bool) -> Project {
    let window = fixture(oracle).snapshot();
    let SessionCellContentSnapshot::Midi(window) = &window.session.cells[0].content else {
        panic!("window");
    };
    let mut snapshot = project.snapshot();
    snapshot.sample_rate = 8_000;
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("midi fixture");
    };
    clip.source = window.source.clone();
    clip.playback_offset_raw = window.playback_offset_raw;
    snapshot.next_id = snapshot.next_id.max(1_000_003);
    Project::from_snapshot(snapshot).unwrap()
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
pub(super) fn start(revision: Arc<PreparedRevision>) -> Playback {
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
pub(super) fn render(playback: &mut Playback, frames: usize) -> Vec<Frame> {
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

#[test]
fn selected_session_off_uses_the_midi_root_policy_in_a_shared_native_receiver() {
    let run = |oracle| {
        let mut editor = Editor::new(fixture(oracle), 16).unwrap();
        let root = editor.project().tracks()[0].id();
        let source = editor.project().track(root).unwrap().source_id().unwrap();
        let device = editor
            .project()
            .track(root)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Instrument,
                    hex_devices::operator_state::OPERATOR_TYPE_ID,
                )
                .unwrap(),
            })
            .unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::AddMidiTrack {
                name: "receiver".into(),
            })
            .unwrap()
        else {
            panic!("receiver");
        };
        editor
            .execute(Command::BindTrackSource {
                track: receiver,
                source: Some(source),
            })
            .unwrap();
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: Some(receiver),
            })
            .unwrap();
        let prepared = revision(editor.project(), Arc::new(hex_devices::SineTable::new()));
        let mut old = start(Arc::clone(&prepared));
        let mut pcm = render(&mut old, 16);
        let mut next = Playback::try_from_prepared_with_session(prepared).unwrap();
        next.try_inherit_runtime_from(&mut old).unwrap();
        pcm.extend(render(&mut next, 1_100));
        pcm
    };
    let expected = run(true);
    assert!(
        expected[..16].iter().flatten().any(|s| *s != 0.0),
        "shared native boundary voice is audible"
    );
    assert!(
        expected[500..].iter().flatten().any(|s| *s != 0.0),
        "shared native source keeps later repeats"
    );
    assert_eq!(
        run(false),
        expected,
        "the receiver's Arrangement owner must not replace the MIDI root's Off policy"
    );
}

#[test]
#[ignore = "optimized timing evidence; run with --release and --ignored"]
fn profile_default_session_reset_and_first_block() -> Result<(), &'static str> {
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        return Err("use --release");
    }
    let prepared = prepared(false, true);
    let clip = prepared.session().midi_clips()[0].id;
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    playback
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    for sample in 0..4 {
        let started = Instant::now();
        for _ in 0..1_000 {
            playback
                .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(0)))
                .unwrap();
            playback.render(black_box(&mut output)).unwrap();
            black_box(&output);
        }
        if sample != 0 {
            eprintln!(
                "default Session reset+first-block sample={sample} rate=8000 tempo=120 block=64 iterations=1000 warmup-samples=1 elapsed={:?}; one mutable source, crossing+boundary intro, prepared scratch; headless only",
                started.elapsed()
            );
        }
    }
    Ok(())
}

fn launch(
    revision: Arc<PreparedRevision>,
    scene: bool,
    explicit: Option<crate::NoteChase>,
) -> Playback {
    let clip = revision.session().midi_clips()[0].id;
    let scene_id = revision.session().scenes()[0].id;
    let quantization = LaunchQuantization::Immediate;
    let command = match (scene, explicit) {
        (false, None) => SessionPlaybackCommand::LaunchClip { clip, quantization },
        (true, None) => SessionPlaybackCommand::LaunchScene {
            scene: scene_id,
            quantization,
        },
        (false, Some(note_chase)) => SessionPlaybackCommand::LaunchClipWithNoteChase {
            clip,
            quantization,
            note_chase,
        },
        (true, Some(note_chase)) => SessionPlaybackCommand::LaunchSceneWithNoteChase {
            scene: scene_id,
            quantization,
            note_chase,
        },
    };
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(command))
        .unwrap();
    playback
}

#[test]
fn explicit_clip_and_scene_launch_choices_match_canonical_boundary_and_repeat_oracles() {
    for (scene, mutable) in [(false, false), (true, true)] {
        let mut off = launch(prepared(false, mutable), scene, Some(crate::NoteChase::Off));
        let mut oracle = launch(prepared(true, mutable), scene, None);
        let expected = render(&mut oracle, 1_100);
        assert!(expected[..16].iter().flatten().any(|s| *s != 0.0));
        assert!(expected[500..].iter().flatten().any(|s| *s != 0.0));
        assert_eq!(render(&mut off, 1_100), expected);
        let mut on = launch(prepared(false, mutable), scene, Some(crate::NoteChase::On));
        let mut raw = launch(prepared(false, mutable), scene, None);
        assert_eq!(render(&mut on, 32), render(&mut raw, 32));
    }
}

#[test]
fn queued_launch_keeps_its_captured_off_choice_until_the_actual_boundary() {
    let actual_revision = prepared(false, false);
    let clip = actual_revision.session().midi_clips()[0].id;
    let mut actual = Playback::try_from_prepared_with_session(actual_revision).unwrap();
    let oracle_revision = prepared(true, false);
    let oracle_clip = oracle_revision.session().midi_clips()[0].id;
    let mut oracle = Playback::try_from_prepared_with_session(oracle_revision).unwrap();
    for playback in [&mut actual, &mut oracle] {
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        // A launch exactly on a grid boundary is immediate. Move inside the
        // first interval so this request actually waits for frame 500.
        assert_eq!(render(playback, 1), vec![[0.0; 2]; 1]);
    }
    actual
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClipWithNoteChase {
                clip,
                quantization: LaunchQuantization::ThirtySecond,
                note_chase: crate::NoteChase::Off,
            },
        ))
        .unwrap();
    oracle
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip: oracle_clip,
                quantization: LaunchQuantization::ThirtySecond,
            },
        ))
        .unwrap();
    actual
        .try_apply_action(PlaybackAction::SetMetronome(false))
        .unwrap();
    assert_eq!(render(&mut actual, 499), vec![[0.0; 2]; 499]);
    assert_eq!(render(&mut oracle, 499), vec![[0.0; 2]; 499]);
    let expected = render(&mut oracle, 600);
    assert!(expected.iter().flatten().any(|s| *s != 0.0));
    assert_eq!(render(&mut actual, 600), expected);
}
