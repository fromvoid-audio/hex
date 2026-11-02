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
