//! Painting uses the existing hybrid overwrite fragments, never a hidden underlay.
use hex_project::{BeatTime, ClipId, Command, Created, Editor, Invalid, Project, TrackId};
fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}
fn fixture(midi: bool) -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "paint".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let command = if midi {
        Command::PlaceMidiClip {
            track,
            start: beat(2),
            length: beat(8),
        }
    } else {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "immutable.wav".into(),
                sample_rate: 48_000,
                frames: 200_000,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        Command::PlaceClip {
            track,
            asset,
            start: 48_000,
            duration: 192_000,
            source_offset: 12_000,
            source_length: 96_000,
            looping: true,
        }
    };
    let Created::Clip(clip) = editor.execute(command).unwrap() else {
        panic!("clip")
    };
    editor
        .execute(Command::RenameClip {
            clip,
            name: "original".into(),
        })
        .unwrap();
    if midi {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(8),
                pitch: 63,
                channel: 2,
                velocity: 91,
            })
            .unwrap();
    } else {
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 24_000,
                fade_out: 24_000,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipGain {
                clip,
                millidb: -6_000,
            })
            .unwrap();
    }
    (editor, track, clip)
}
#[test]
fn clip_paint_cuts_audio_and_midi_with_source_mapping_boundaries_and_one_undo() {
    // Interior, left/right overlap, whole replacement, touching and one beat cell.
    for midi in [false, true] {
        for (start, end) in [(4, 5), (0, 4), (6, 12), (2, 10), (10, 11), (2, 3)] {
            let (mut editor, track, original) = fixture(midi);
            let before = editor.project().clone();
            let Created::Clip(painted) = editor
                .execute(Command::PlaceMidiClipWithOverwrite {
                    track,
                    start: beat(start),
                    length: beat(end - start),
                })
                .unwrap()
            else {
                panic!("painted clip")
            };
            assert_ne!(painted, original);
            let project = editor.project();
            let new = project.midi_clip(painted).unwrap();
            assert_eq!(
                (new.start(), new.length()),
                (beat(start), beat(end - start))
            );
            assert!(new.notes().is_empty());
            let row = project.track(track).unwrap();
            let mut bounds: Vec<_> = row
                .audio_clips()
                .iter()
                .map(|c| (c.start(), c.start() + c.duration()))
                .chain(row.midi_clips().iter().map(|c| {
                    (
                        project.tempo_map().beat_to_frame(c.start()).unwrap(),
                        project
                            .tempo_map()
                            .beat_to_frame(BeatTime::from_raw(c.start().raw() + c.length().raw()))
                            .unwrap(),
                    )
                }))
                .collect();
            bounds.sort_unstable();
            let mut expected = vec![(start * 24_000, end * 24_000)];
            if start > 2 {
                expected.push((48_000, start.min(10) * 24_000));
            }
            if end < 10 {
                expected.push((end.max(2) * 24_000, 240_000));
            }
            expected.retain(|(a, b)| a < b);
            expected.sort_unstable();
            assert_eq!(bounds, expected, "midi={midi} range={start}..{end}");
            assert!(bounds.windows(2).all(|w| w[0].1 <= w[1].0));
            if midi {
                for fragment in row.midi_clips().iter().filter(|c| c.id() != painted) {
                    assert_eq!(fragment.name(), "original");
                    assert!(!fragment.notes().is_empty());
                    for note in fragment.notes() {
                        assert_eq!((note.pitch(), note.channel(), note.velocity()), (63, 2, 91));
                    }
                }
            } else {
                let old = before.clip(original).unwrap();
                for fragment in row.audio_clips() {
                    assert_eq!(fragment.name(), old.name());
                    assert_eq!(fragment.asset(), old.asset());
                    assert_eq!(fragment.source_offset(), 12_000);
                    assert_eq!(fragment.source_length(), 96_000);
                    assert_eq!(
                        fragment.source_phase(),
                        (fragment.start() - 48_000) % 96_000
                    );
                    assert_eq!(fragment.gain(), old.gain());
                    // The original fade curve stays anchored at the original time.
                    for local in [0, fragment.duration() / 2, fragment.duration() - 1] {
                        assert_eq!(
                            fragment.fade().gain_at(local),
                            old.fade().gain_at(fragment.start() - 48_000 + local)
                        );
                    }
                }
            }
            let after = project.snapshot();
            assert_eq!(
                Project::from_snapshot(after.clone()).unwrap().snapshot(),
                after
            );
            assert!(editor.undo());
            assert_eq!(editor.project().tracks(), before.tracks());
            assert_eq!(editor.project().assets(), before.assets());
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}
#[test]
fn clip_paint_refuses_invalid_stale_and_failed_preparation_without_partial_cuts() {
    let (mut editor, track, _) = fixture(false);
    let Created::Track(stale) = editor
        .execute(Command::AddTrack {
            name: "stale".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for (target, start, length) in [
        (track, beat(4), BeatTime::ZERO),
        (track, BeatTime::from_raw(u64::MAX), BeatTime::ONE),
        (stale, beat(4), beat(1)),
    ] {
        assert!(
            editor
                .execute(Command::PlaceMidiClipWithOverwrite {
                    track: target,
                    start,
                    length
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    let command = Command::PlaceMidiClipWithOverwrite {
        track,
        start: beat(4),
        length: beat(1),
    };
    let result: Result<_, Invalid> = editor.execute_validated(command.clone(), |candidate| {
        assert_eq!(candidate.track(track).unwrap().audio_clips().len(), 2);
        Err(Invalid("injected preparation refusal"))
    });
    assert!(result.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    let mut exhausted = before.clone();
    exhausted.next_id = u64::MAX;
    let mut exhausted = Editor::new(Project::from_snapshot(exhausted).unwrap(), 8).unwrap();
    let before = exhausted.project().snapshot();
    assert!(exhausted.execute(command).is_err());
    assert_eq!(exhausted.project().snapshot(), before);
    assert!(!exhausted.can_undo());
}
