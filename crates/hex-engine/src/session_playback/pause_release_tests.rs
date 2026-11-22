//! Pause releases every held instrument note; resume never re-sounds a note
//! that was held at the pause (Pulse #4139). Only new note-ons play.
use super::source_loop_tests::prepare_mutable;
use super::*;

const Q: u64 = 1_u64 << 32;
/// One beat at the fixture's 120 bpm and 48 kHz.
const BEAT_FRAMES: usize = 24_000;
const PAUSE_AT: usize = BEAT_FRAMES / 4;

#[derive(Clone, Copy, Debug)]
enum Instrument {
    Analytic,
    Mutable,
}

#[derive(Clone, Copy, Debug)]
enum Owner {
    Arrangement,
    Session,
}

/// A note held from frame 0 across the pause, then a new note-on at beat 1:
/// the next loop iteration (looped clip) or a second note (linear clip). The
/// Arrangement clip and the launched Session clip carry the same material.
fn held_note_project(looped: bool, instrument: Instrument) -> Project {
    let mut snapshot = project(1.0, 0, 3 * Q / 4).snapshot();
    let next_id = snapshot.next_id;
    let track = snapshot.tracks.first_mut().expect("fixture track");
    if matches!(instrument, Instrument::Mutable) {
        track.instrument.as_mut().expect("fixture instrument").state =
            hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
                1,
                vec![0],
            )
            .expect("valid test instrument state");
    }
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = midi_clips.first_mut().expect("fixture Arrangement clip");
    clip.length_raw = 2 * Q;
    let second = |id| MidiNoteSnapshot {
        active: true,
        id,
        start_raw: Q,
        duration_raw: Q / 4,
        pitch: 60,
        channel: 0,
        velocity: 100,
    };
    if looped {
        clip.looping = true;
        clip.loop_start_raw = 0;
        clip.loop_end_raw = Q;
    } else {
        clip.notes.push(second(next_id));
    }
    let Some(SessionCellContentSnapshot::Midi(cell)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    if looped {
        cell.source.looping = true;
        cell.source.loop_start_raw = 0;
        cell.source.loop_end_raw = Q;
    } else {
        cell.source.notes.push(second(next_id + 1));
    }
    snapshot.next_id = next_id + 2;
    legacy_project_from_snapshot(snapshot).expect("valid held-note project")
}

fn prepared(project: &Project, instrument: Instrument) -> Arc<PreparedRevision> {
    match instrument {
        Instrument::Analytic => Arc::new(
            PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .expect("analytic fixture prepares"),
        ),
        Instrument::Mutable => prepare_mutable(project),
    }
}

fn render_frames(playback: &mut SessionPlayback, frames: usize, block: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for chunk in output.chunks_mut(block) {
        playback.render(chunk).expect("render");
    }
    output
}

fn voices_idle(playback: &SessionPlayback) -> bool {
    playback.release_voice_count() == 0
        && playback
            .mutable_instruments
            .iter()
            .flatten()
            .all(|runtime| runtime.committed_is_quiescent())
}

#[test]
fn pause_releases_held_notes_and_resume_plays_only_new_note_ons() {
    for owner in [Owner::Arrangement, Owner::Session] {
        for instrument in [Instrument::Analytic, Instrument::Mutable] {
            for looped in [false, true] {
                for block in [1, 64, 512] {
                    let case = format!("{owner:?} {instrument:?}, looped={looped}, block={block}");
                    let project = held_note_project(looped, instrument);
                    let revision = prepared(&project, instrument);
                    let session_clip = revision.session().midi_clips()[0].id;
                    let mut playback = SessionPlayback::new(revision).unwrap();
                    // Pause cuts gates on source-owner runtimes; every
                    // instrument track binds a native source, so that is all.
                    assert!(
                        playback
                            .mutable_instruments
                            .iter()
                            .flatten()
                            .all(|runtime| runtime.is_source_owner()),
                        "{case}: instrument runtimes are source owners"
                    );
                    if matches!(owner, Owner::Session) {
                        playback
                            .apply(SessionPlaybackCommand::LaunchClip {
                                clip: session_clip,
                                quantization: LaunchQuantization::Immediate,
                            })
                            .unwrap();
                    }
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    let held = render_frames(&mut playback, PAUSE_AT, block);
                    assert!(
                        held[PAUSE_AT - 64..].iter().any(|frame| *frame != [0.0; 2]),
                        "{case}: the note sounds before the pause"
                    );
                    playback.apply(SessionPlaybackCommand::Pause).unwrap();
                    // The held voice releases while paused, then goes idle.
                    let paused = render_frames(&mut playback, BEAT_FRAMES, block);
                    assert!(
                        paused[..64].iter().any(|frame| *frame != [0.0; 2]),
                        "{case}: the release plays out after the pause"
                    );
                    assert!(
                        paused[BEAT_FRAMES - 512..]
                            .iter()
                            .all(|frame| *frame == [0.0; 2]),
                        "{case}: the release ends while paused"
                    );
                    assert!(voices_idle(&playback), "{case}: every voice is idle");
                    assert_eq!(playback.performance_frame(), PAUSE_AT as u64);
                    // Resume: the note held at the pause is not re-sounded.
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    let resumed = render_frames(&mut playback, BEAT_FRAMES - PAUSE_AT, block);
                    assert!(
                        resumed.iter().all(|frame| *frame == [0.0; 2]),
                        "{case}: no voice for a note held across the pause"
                    );
                    // A new note-on after the pause plays normally.
                    let next = render_frames(&mut playback, BEAT_FRAMES / 8, block);
                    assert!(
                        next.iter().any(|frame| *frame != [0.0; 2]),
                        "{case}: the next note-on sounds"
                    );
                }
            }
        }
    }
}

fn operator_project(
    first_duration: u64,
    second_start: u64,
    state: hex_lattice_contract::NativeDeviceState,
) -> Project {
    let mut snapshot = project(1.0, 0, first_duration).snapshot();
    let next_id = snapshot.next_id;
    let track = snapshot.tracks.first_mut().expect("fixture track");
    track.instrument.as_mut().expect("fixture instrument").state = state;
    let (TrackContentSnapshot::Hybrid { midi_clips, .. }
    | TrackContentSnapshot::Instrument(midi_clips)) = &mut track.content
    else {
        panic!("fixture Arrangement MIDI clip")
    };
    let clip = midi_clips.first_mut().expect("fixture Arrangement clip");
    clip.length_raw = second_start + Q;
    let second = |id| MidiNoteSnapshot {
        active: true,
        id,
        start_raw: second_start,
        duration_raw: Q / 4,
        pitch: 64,
        channel: 0,
        velocity: 100,
    };
    clip.notes.push(second(next_id));
    let Some(SessionCellContentSnapshot::Midi(cell)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture Session MIDI clip")
    };
    cell.source.source_length_raw = second_start + Q;
    cell.source.loop_end_raw = second_start + Q;
    cell.source.notes.push(second(next_id + 1));
    snapshot.next_id = next_id + 2;
    legacy_project_from_snapshot(snapshot).expect("valid operator project")
}

fn patch_operator(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
    value: hex_lattice_contract::ParameterValue,
) -> hex_lattice_contract::NativeDeviceState {
    crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        state,
        parameter,
        value,
    )
    .expect("valid operator parameter patch")
}

fn prepare_operator(project: &Project) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .expect("operator fixture prepares"),
    )
}

fn handoff_operator(previous: &mut SessionPlayback, project: &Project) -> SessionPlayback {
    let mut next = SessionPlayback::new(prepare_operator(project)).expect("candidate playback");
    next.try_inherit_state_live_from(previous)
        .expect("parameter handoff succeeds");
    next
}

#[test]
fn paused_session_parameter_drags_do_not_retrigger_suppressed_note_tails() {
    use hex_devices::operator_state::parameter::{EnvelopeField, envelope};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let base = crate::operator_legacy_test::legacy_operator_state();
    let base = patch_operator(
        &base,
        envelope(1, EnvelopeField::Release),
        ParameterValue::Float(0.02),
    );
    let initial = operator_project(3 * Q / 4, 2 * Q, base.clone());
    let revision = prepare_operator(&initial);
    let session_clip = revision.session().midi_clips()[0].id;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let held = render_frames(&mut playback, PAUSE_AT, 256);
    assert!(held[PAUSE_AT - 64..].iter().any(|f| *f != [0.0; 2]));

    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    let drained = render_frames(&mut playback, 4_096, 256);
    assert!(drained[..64].iter().any(|f| *f != [0.0; 2]));
    assert!(
        drained[drained.len() - 512..]
            .iter()
            .all(|f| *f == [0.0; 2])
    );

    // Simulate dragging Operator A level across multiple live parameter edits while paused.
    let level_a = ParameterId::from_static(2);
    let mut state = base;
    for (step, level) in [0.9_f32, 0.8, 0.7, 0.6].into_iter().enumerate() {
        state = patch_operator(&state, level_a, ParameterValue::Float(level));
        let edited = operator_project(3 * Q / 4, 2 * Q, state.clone());
        playback = handoff_operator(&mut playback, &edited);
        let paused = render_frames(&mut playback, 1_024, 256);
        assert!(
            paused.iter().all(|f| *f == [0.0; 2]),
            "step {step}: adjusting parameter while paused must stay silent"
        );
    }

    // Resuming must not re-sound the paused held note before the second note-on.
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let resumed = render_frames(&mut playback, BEAT_FRAMES - PAUSE_AT, 256);
    assert!(
        resumed.iter().all(|f| *f == [0.0; 2]),
        "paused note must stay suppressed on resume after parameter edits"
    );
}

#[test]
fn increasing_release_while_paused_or_playing_does_not_resurrect_finished_notes() {
    use hex_devices::operator_state::parameter::{EnvelopeField, envelope};
    use hex_lattice_contract::ParameterValue;

    let base = crate::operator_legacy_test::legacy_operator_state();
    let short_release = patch_operator(
        &base,
        envelope(1, EnvelopeField::Release),
        ParameterValue::Float(0.01),
    );
    // Note 1: 0..6000 frames (1/4 beat), ends at ~6480 frames. Second note at 2 beats (48000).
    let initial = operator_project(Q / 4, 2 * Q, short_release.clone());

    for owner in [Owner::Arrangement, Owner::Session] {
        for paused in [true, false] {
            let case = format!("{owner:?}, paused={paused}");
            let revision = prepare_operator(&initial);
            let session_clip = revision.session().midi_clips()[0].id;
            let mut playback = SessionPlayback::new(revision).unwrap();
            if matches!(owner, Owner::Session) {
                playback
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip: session_clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
            }
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            // Advance to frame 12000, well after Note 1 finished (~6480).
            let prefix = render_frames(&mut playback, 12_000, 256);
            assert!(prefix[..1_024].iter().any(|f| *f != [0.0; 2]), "{case}");
            assert!(
                prefix[prefix.len() - 512..].iter().all(|f| *f == [0.0; 2]),
                "{case}: Note 1 already finished"
            );
            if paused {
                playback.apply(SessionPlaybackCommand::Pause).unwrap();
                let settled = render_frames(&mut playback, 512, 256);
                assert!(settled.iter().all(|f| *f == [0.0; 2]), "{case}");
            }

            // Drag Release up to 1.0s and 1.2s so Note 1's new release span covers frame 12000+.
            let mut state = short_release.clone();
            for (step, release) in [1.0_f32, 1.2].into_iter().enumerate() {
                state = patch_operator(
                    &state,
                    envelope(1, EnvelopeField::Release),
                    ParameterValue::Float(release),
                );
                let edited = operator_project(Q / 4, 2 * Q, state.clone());
                playback = handoff_operator(&mut playback, &edited);
                let after = render_frames(&mut playback, 2_048, 256);
                assert!(
                    after.iter().all(|f| *f == [0.0; 2]),
                    "{case}, step {step}: increasing Release must not resurrect already-finished note"
                );
            }
            if paused {
                playback.apply(SessionPlaybackCommand::Play).unwrap();
                let resumed = render_frames(&mut playback, 4_096, 256);
                assert!(
                    resumed.iter().all(|f| *f == [0.0; 2]),
                    "{case}: resuming must not sound resurrected note tail"
                );
            }
        }
    }
}

#[test]
fn paused_seek_parameter_edits_and_mutable_import_do_not_sound_while_paused() {
    use hex_devices::operator_state::parameter::{EnvelopeField, envelope, feedback};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let base = crate::operator_legacy_test::legacy_operator_state();
    let short_release = patch_operator(
        &base,
        envelope(1, EnvelopeField::Release),
        ParameterValue::Float(0.05),
    );
    let initial = operator_project(3 * Q / 4, 2 * Q, short_release.clone());
    let track_id = initial.tracks()[0].id();

    for owner in [Owner::Arrangement, Owner::Session] {
        let case = format!("{owner:?}");
        let keyboard = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        let prepare_with_keyboard = |project: &Project| {
            let revision = prepare_operator(project);
            let mut pb =
                crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
            pb.prepare_computer_keyboard(project, track_id, Arc::clone(&keyboard))
                .expect("keyboard attaches");
            pb
        };
        let render_pb = |pb: &mut crate::playback::Playback, frames: usize| {
            let mut output = vec![[0.0; 2]; frames];
            for chunk in output.chunks_mut(256) {
                pb.render(chunk).expect("render");
            }
            output
        };
        let mut playback = prepare_with_keyboard(&initial);
        if matches!(owner, Owner::Session) {
            let session_clip = playback.prepared_revision().unwrap().session().midi_clips()[0].id;
            playback
                .try_apply_action(crate::PlaybackAction::Session(
                    SessionPlaybackCommand::LaunchClip {
                        clip: session_clip,
                        quantization: LaunchQuantization::Immediate,
                    },
                ))
                .unwrap();
        }
        // Seek into the middle of Note 1 while paused (without ever playing).
        playback
            .try_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Seek(PAUSE_AT as u64),
            ))
            .unwrap();
        assert!(!playback.playing());
        let before = render_pb(&mut playback, 512);
        assert!(before.iter().all(|f| *f == [0.0; 2]), "{case}");

        // Analytic parameter edit while paused at cursor inside a note.
        let level_edited_state = patch_operator(
            &short_release,
            ParameterId::from_static(2),
            ParameterValue::Float(0.75),
        );
        let level_edited = operator_project(3 * Q / 4, 2 * Q, level_edited_state.clone());
        let mut next = prepare_with_keyboard(&level_edited);
        next.try_inherit_runtime_from(&mut playback)
            .expect("analytic edit handoff");
        playback = next;
        let after_analytic = render_pb(&mut playback, 1_024);
        assert!(
            after_analytic.iter().all(|f| *f == [0.0; 2]),
            "{case}: paused analytic parameter edit after Seek must stay silent"
        );

        // Switching analytic -> mutable (enabling Feedback) while paused at cursor inside a note.
        let mutable_state = patch_operator(
            &level_edited_state,
            feedback(1),
            ParameterValue::Float(0.25),
        );
        let mutable_edited = operator_project(3 * Q / 4, 2 * Q, mutable_state);
        let mut next = prepare_with_keyboard(&mutable_edited);
        next.try_inherit_runtime_from(&mut playback)
            .expect("mutable import handoff");
        playback = next;
        let after_mutable = render_pb(&mut playback, 1_024);
        assert!(
            after_mutable.iter().all(|f| *f == [0.0; 2]),
            "{case}: paused analytic->mutable parameter edit after Seek must stay silent"
        );
    }
}
