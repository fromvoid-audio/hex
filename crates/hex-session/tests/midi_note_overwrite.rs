//! Real worker/preparation, independent literal playback and current-format IO.
use hex_engine::{
    LaunchQuantization, PlaybackAction, PlaybackCommand, PreparedRevision, SessionPlaybackCommand,
    SineTable, playback::Playback,
};
use hex_project::{
    BeatTime, ClipId, Command, Created, MidiNoteInsert, MidiNoteSnapshot, Project, SceneId,
    SessionCellContentSnapshot, Snapshot, TrackContentSnapshot,
};
use hex_session::service::{Event, Request, Service};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}
fn fraction(n: u64, d: u64) -> BeatTime {
    BeatTime::from_ratio(n, d).unwrap()
}
fn receive(service: &Service) -> Event {
    let until = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < until, "real worker did not complete");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn success(service: &Service) -> Event {
    let event = receive(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn edit(service: &Service, command: Command) -> Event {
    service.submit(Request::Edit(command)).unwrap();
    success(service)
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-note-overwrite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    service: Service,
    clip: ClipId,
    scene: Option<SceneId>,
    before: Event,
    directory: Directory,
}
impl Fixture {
    fn new(console: bool) -> Self {
        let directory = Directory::new();
        let service = Service::new(8_000, 32, 8_000_000).unwrap();
        success(&service);
        let Some(Created::Track(track)) = edit(
            &service,
            Command::InsertTrackWithDevice {
                name: "literal note oracle".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            },
        )
        .created
        else {
            panic!("track")
        };
        let scene = console.then(|| {
            let Some(Created::Scene(scene)) =
                edit(&service, Command::AddScene { name: "one".into() }).created
            else {
                panic!("scene")
            };
            scene
        });
        let clip = if let Some(scene) = scene {
            let Some(Created::SessionClip(clip)) = edit(
                &service,
                Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "notes".into(),
                    source_length: beat(4),
                },
            )
            .created
            else {
                panic!("console clip")
            };
            clip
        } else {
            let Some(Created::Clip(clip)) = edit(
                &service,
                Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: beat(4),
                },
            )
            .created
            else {
                panic!("arrangement clip")
            };
            clip
        };
        let before = edit(
            &service,
            Command::AddMidiNotes {
                clip,
                notes: vec![
                    MidiNoteInsert {
                        active: true,
                        start: BeatTime::ZERO,
                        duration: beat(3),
                        pitch: 60,
                        channel: 3,
                        velocity: 91,
                    },
                    MidiNoteInsert {
                        active: true,
                        start: fraction(7, 2),
                        duration: fraction(1, 4),
                        pitch: 60,
                        channel: 7,
                        velocity: 103,
                    },
                    MidiNoteInsert {
                        active: true,
                        start: beat(2),
                        duration: fraction(1, 4),
                        pitch: 64,
                        channel: 1,
                        velocity: 77,
                    },
                ],
            },
        );
        Self {
            service,
            clip,
            scene,
            before,
            directory,
        }
    }
    fn move_note(&self, duration: BeatTime) -> Command {
        let source = self.before.state.project.midi_source(self.clip).unwrap();
        Command::UpdateMidiNote {
            note: source.notes()[1].id(),
            start: BeatTime::ONE,
            duration,
            pitch: 60,
            channel: 7,
            velocity: 103,
        }
    }
}
fn notes(snapshot: &mut Snapshot, console: bool) -> &mut Vec<MidiNoteSnapshot> {
    if console {
        let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
            panic!("midi cell")
        };
        &mut clip.source.notes
    } else {
        let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content
        else {
            panic!("hybrid")
        };
        &mut midi_clips[0].notes
    }
}
fn prepared(event: &Event) -> Arc<PreparedRevision> {
    event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .unwrap()
        .clone()
}
fn render(revision: Arc<PreparedRevision>, scene: Option<SceneId>, block: usize) -> Vec<[f32; 2]> {
    render_frames(revision, scene, block, 24_000)
}
fn render_frames(
    revision: Arc<PreparedRevision>,
    scene: Option<SceneId>,
    block: usize,
    frames: usize,
) -> Vec<[f32; 2]> {
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    if let Some(scene) = scene {
        playback
            .try_apply_action(PlaybackAction::LaunchAndPlay(
                SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
    } else {
        playback.apply(PlaybackCommand::Play);
    }
    let mut output = vec![[0.0; 2]; frames];
    for chunk in output.chunks_mut(block) {
        playback.render(chunk).unwrap();
    }
    output
}
fn same_pcm(a: &[[f32; 2]], b: &[[f32; 2]]) {
    assert_eq!(a.len(), b.len());
    let max = a
        .iter()
        .zip(b)
        .flat_map(|(a, b)| a.iter().zip(b))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(max < 1.0e-5, "independent literal playback error {max}");
}

#[test]
fn worker_refusal_commit_history_literal_pcm_and_saved_roundtrip_agree() {
    for console in [false, true] {
        let f = Fixture::new(console);
        // Numeric geometry is valid, but this one-tick gate collapses at8kHz.
        // Preparation must reject AFTER planning the old prefix, not publish it.
        f.service
            .submit(Request::Edit(f.move_note(BeatTime::from_raw(1))))
            .unwrap();
        let refused = receive(&f.service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, f.before.state.project);
        assert_eq!(refused.state.revision, f.before.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (f.before.state.can_undo, f.before.state.can_redo)
        );
        assert!(refused.playback.is_none());

        let accepted = edit(&f.service, f.move_note(fraction(1, 2)));
        let mut literal = f.before.state.project.snapshot();
        let expected = notes(&mut literal, console);
        expected[0].duration_raw = BeatTime::ONE.raw();
        expected[1].start_raw = BeatTime::ONE.raw();
        expected[1].duration_raw = fraction(1, 2).raw();
        let literal = Project::from_snapshot(literal).unwrap();
        assert_eq!(&*accepted.state.project, &literal);
        let independent = Arc::new(
            PreparedRevision::prepare(&literal, &BTreeMap::new(), Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let actual = render(prepared(&accepted), f.scene, 257);
        let expected = render(independent, f.scene, 73);
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-4));
        same_pcm(&actual, &expected);
        if !console {
            let revision = prepared(&accepted);
            let mut gates: Vec<_> = revision
                .midi_preview()
                .get(f.clip)
                .unwrap()
                .samples
                .iter()
                .map(|note| (note.frame_start, note.frame_gate_end, note.pitch))
                .collect();
            gates.sort_unstable();
            assert_eq!(
                gates,
                vec![(0, 4_000, 60), (4_000, 6_000, 60), (8_000, 9_000, 64)]
            );
        }
        f.service.submit(Request::Undo).unwrap();
        let undo = success(&f.service);
        assert_eq!(undo.state.project, f.before.state.project);
        same_pcm(
            &render(prepared(&undo), f.scene, 61),
            &render(prepared(&f.before), f.scene, 127),
        );
        f.service.submit(Request::Redo).unwrap();
        let redo = success(&f.service);
        assert_eq!(redo.state.project, accepted.state.project);
        same_pcm(&render(prepared(&redo), f.scene, 137), &actual);
        let path = f.directory.0.join("notes-current.hex");
        f.service.submit(Request::SaveNew(path.clone())).unwrap();
        let saved = success(&f.service);
        assert!(saved.save_complete && !saved.state.unsaved);
        assert_eq!(hex_storage::load(&path).unwrap(), literal);
        edit(&f.service, Command::MasterGain { gain: 0.5 });
        f.service.submit(Request::Open(path)).unwrap();
        let opened = success(&f.service);
        assert!(opened.project_replaced && opened.open_complete);
        assert_eq!(opened.state.project, accepted.state.project);
        same_pcm(&render(prepared(&opened), f.scene, 89), &actual);
    }
}

#[test]
fn invalid_current_saved_source_refuses_without_rewrite_or_working_history_loss() {
    for console in [false, true] {
        let f = Fixture::new(console);
        let accepted = edit(&f.service, f.move_note(fraction(1, 2)));
        let path = f.directory.0.join("working.hex");
        f.service.submit(Request::SaveNew(path.clone())).unwrap();
        success(&f.service);
        let original = fs::read(&path).unwrap();
        let mut document: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
        let pointer = if console {
            "/session/cells/0/content/clip/source/notes"
        } else {
            "/tracks/0/midi_clips/0/notes"
        };
        let notes = document
            .pointer_mut(pointer)
            .unwrap()
            .as_array_mut()
            .unwrap();
        notes[1]["start_raw"] = serde_json::json!(fraction(1, 2).raw());
        let invalid = serde_json::to_vec_pretty(&document).unwrap();
        let bad_path = f.directory.0.join("overlapping-source.hex");
        fs::write(&bad_path, &invalid).unwrap();
        edit(&f.service, Command::MasterGain { gain: 0.5 });
        f.service.submit(Request::Undo).unwrap();
        let working = success(&f.service);
        assert!(working.state.can_redo);
        assert_eq!(working.state.project, accepted.state.project);
        f.service.submit(Request::Open(bad_path.clone())).unwrap();
        let refused = receive(&f.service);
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|error| error.contains("same pitch must not overlap")),
            "{:?}",
            refused.error
        );
        assert!(!refused.project_replaced);
        assert_eq!(refused.state.project, working.state.project);
        assert_eq!(refused.state.revision, working.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (working.state.can_undo, working.state.can_redo)
        );
        assert_eq!(fs::read(bad_path).unwrap(), invalid);
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
fn source_range_worker_prepares_literal_pcm_history_and_saved_extent_in_both_contexts() {
    for console in [false, true] {
        let f = Fixture::new(console);
        let track = f.before.state.project.tracks()[0].id();
        // The other clip context shares the hybrid track but must retain its own source.
        let other = if console {
            let Some(Created::Clip(other)) = edit(
                &f.service,
                Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: beat(4),
                },
            )
            .created
            else {
                panic!("arrangement counterpart")
            };
            other
        } else {
            let Some(Created::Scene(scene)) = edit(
                &f.service,
                Command::AddScene {
                    name: "counterpart".into(),
                },
            )
            .created
            else {
                panic!("counterpart scene")
            };
            let Some(Created::SessionClip(other)) = edit(
                &f.service,
                Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "counterpart".into(),
                    source_length: beat(4),
                },
            )
            .created
            else {
                panic!("console counterpart")
            };
            other
        };
        edit(
            &f.service,
            Command::AddMidiNotes {
                clip: other,
                notes: vec![MidiNoteInsert {
                    active: true,
                    start: fraction(1, 2),
                    duration: fraction(1, 4),
                    pitch: 72,
                    channel: 0,
                    velocity: 53,
                }],
            },
        );
        let source_command = if console {
            Command::SetSessionMidiClipSource {
                clip: f.clip,
                source_length: beat(4),
                source_start: fraction(1, 2),
                loop_start: beat(1),
                loop_end: fraction(15, 4),
                looping: false,
            }
        } else {
            Command::SetMidiClipSource {
                clip: f.clip,
                source_length: beat(4),
                source_start: fraction(1, 2),
                loop_start: beat(1),
                loop_end: fraction(15, 4),
                looping: false,
                length: beat(10),
            }
        };
        edit(&f.service, source_command);
        edit(&f.service, Command::MasterGain { gain: 0.5 });
        f.service.submit(Request::Undo).unwrap();
        let before = success(&f.service);
        assert!(before.state.can_redo);
        for (start, end) in [
            (beat(2), beat(2)),
            (BeatTime::ZERO, BeatTime::from_raw(u64::MAX)),
            // Canonical geometry is positive, but the copied one-tick gate
            // collapses at8kHz. No planned prefix or identity may be published.
            (BeatTime::ONE, BeatTime::from_raw(BeatTime::ONE.raw() + 1)),
        ] {
            f.service
                .submit(Request::Edit(Command::DuplicateMidiSourceRange {
                    clip: f.clip,
                    start,
                    end,
                }))
                .unwrap();
            let refused = receive(&f.service);
            assert!(refused.error.is_some());
            assert_eq!(refused.state.project, before.state.project);
            assert_eq!(refused.state.revision, before.state.revision);
            assert_eq!(
                (refused.state.can_undo, refused.state.can_redo),
                (before.state.can_undo, before.state.can_redo)
            );
            assert!(refused.playback.is_none());
        }
        let accepted = edit(
            &f.service,
            Command::DuplicateMidiSourceRange {
                clip: f.clip,
                start: beat(1),
                end: beat(4),
            },
        );
        assert_eq!(accepted.state.revision, before.state.revision + 1);
        assert!(!accepted.state.can_redo);
        let first = before.state.project.next_identity();
        assert_eq!(
            accepted.created,
            Some(Created::Notes {
                clip: f.clip,
                first: accepted.state.project.midi_source(f.clip).unwrap().notes()[3].id(),
                last: accepted.state.project.midi_source(f.clip).unwrap().notes()[5].id(),
            })
        );
        let mut literal = before.state.project.snapshot();
        let old = notes(&mut literal, console).clone();
        // Literal expected intersections for [1,4), translated by3 beats.
        // Preserve the final quarter beat of copied silence through source end7.
        for (index, start, duration) in [
            (0, beat(4), beat(2)),
            (1, fraction(13, 2), fraction(1, 4)),
            (2, beat(5), fraction(1, 4)),
        ] {
            notes(&mut literal, console).push(MidiNoteSnapshot {
                active: true,
                id: first + index as u64,
                start_raw: start.raw(),
                duration_raw: duration.raw(),
                pitch: old[index].pitch,
                channel: old[index].channel,
                velocity: old[index].velocity,
            });
        }
        if console {
            let SessionCellContentSnapshot::Midi(clip) = &mut literal.session.cells[0].content
            else {
                panic!("midi cell")
            };
            clip.source.source_length_raw = beat(7).raw();
        } else {
            let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut literal.tracks[0].content
            else {
                panic!("hybrid")
            };
            midi_clips[0].source_length_raw = beat(7).raw();
        }
        literal.next_id = first + 3;
        let literal = Project::from_snapshot(literal).unwrap();
        assert_eq!(&*accepted.state.project, &literal);
        assert_eq!(
            accepted.state.project.midi_source(other),
            before.state.project.midi_source(other)
        );
        let independent = Arc::new(
            PreparedRevision::prepare(&literal, &BTreeMap::new(), Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let actual = render_frames(prepared(&accepted), f.scene, 257, 40_000);
        let expected = render_frames(independent, f.scene, 73, 40_000);
        assert!(
            actual[16_000..28_000]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 1.0e-4)
        );
        same_pcm(&actual, &expected);
        f.service.submit(Request::Undo).unwrap();
        let undo = success(&f.service);
        let mut expected_undo = before.state.project.snapshot();
        expected_undo.next_id = first + 3;
        assert_eq!(undo.state.project.snapshot(), expected_undo);
        same_pcm(
            &render_frames(prepared(&undo), f.scene, 61, 40_000),
            &render_frames(prepared(&before), f.scene, 127, 40_000),
        );
        f.service.submit(Request::Redo).unwrap();
        let redo = success(&f.service);
        assert_eq!(redo.state.project, accepted.state.project);
        same_pcm(
            &render_frames(prepared(&redo), f.scene, 137, 40_000),
            &actual,
        );
        edit(&f.service, Command::MasterGain { gain: 0.5 });
        f.service.submit(Request::Undo).unwrap();
        let before_noop = success(&f.service);
        let noop = edit(
            &f.service,
            Command::DuplicateMidiSourceRange {
                clip: f.clip,
                start: beat(6),
                end: fraction(13, 2),
            },
        );
        assert_eq!(noop.state.project, before_noop.state.project);
        assert_eq!(noop.state.revision, before_noop.state.revision);
        assert!(noop.state.can_redo);
        assert!(noop.playback.is_none());
        let path = f.directory.0.join("range-current.hex");
        f.service.submit(Request::SaveNew(path.clone())).unwrap();
        let saved = success(&f.service);
        assert!(saved.save_complete && !saved.state.unsaved);
        assert_eq!(hex_storage::load(&path).unwrap(), literal);
        edit(&f.service, Command::MasterGain { gain: 0.5 });
        f.service.submit(Request::Open(path)).unwrap();
        let opened = success(&f.service);
        assert!(opened.project_replaced && opened.open_complete);
        assert_eq!(opened.state.project, accepted.state.project);
        same_pcm(
            &render_frames(prepared(&opened), f.scene, 89, 40_000),
            &actual,
        );
    }
}
