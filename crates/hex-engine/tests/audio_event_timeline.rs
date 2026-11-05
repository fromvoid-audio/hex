//! Free event placement renders only the clip window, at its placed song time.
use hex_engine::{Audio, Frame, Renderer, compile};
use hex_project::{AudioEventEdit, ClipId, Command, Created, Editor, Project, TrimEdge};
use std::collections::BTreeMap;

const SOURCE: u64 = 100;

fn value(frame: u64) -> f32 {
    (frame + 1) as f32 / 1024.0
}

fn imported() -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "test".into(),
            path: "test.wav".into(),
            sample_rate: 48_000,
            frames: SOURCE,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    (editor, clip)
}

fn render(editor: &Editor, frames: usize, block: usize) -> Vec<Frame> {
    let pcm = Audio::new(48_000, (0..SOURCE).map(|f| [value(f), -value(f)]).collect()).unwrap();
    let media = BTreeMap::from([(editor.project().assets().next().unwrap().id(), pcm)]);
    let compiled = compile::project(editor.project(), &media).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = vec![[0.0; 2]; frames];
    for chunk in output.chunks_mut(block) {
        renderer.render(chunk).unwrap();
    }
    output
}

#[test]
fn placed_events_stay_silent_outside_the_window_and_play_at_song_time_when_widened() {
    let (mut editor, clip) = imported();
    let original = render(&editor, 200, 200);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 2,
                start: 130,
            },
        })
        .unwrap();
    for block in [1, 7, 64, 200] {
        let placed = render(&editor, 200, block);
        // The grouped conversion keeps the untouched event bit-identical.
        assert_eq!(placed[..40], original[..40], "block {block}");
        assert!(
            placed[40..].iter().all(|f| *f == [0.0, 0.0]),
            "block {block}"
        );
    }
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 190,
        })
        .unwrap();
    for block in [1, 7, 64, 200] {
        let widened = render(&editor, 200, block);
        for (frame, sample) in widened.iter().enumerate() {
            let frame = frame as u64;
            let expected = match frame {
                0..40 => [value(frame), -value(frame)],
                130..190 => [value(frame - 90), -value(frame - 90)],
                _ => [0.0, 0.0],
            };
            assert_eq!(*sample, expected, "block {block} frame {frame}");
        }
    }
}

#[test]
fn negative_event_placement_retains_pcm_and_reveals_it_at_the_same_song_time_on_left_extension() {
    let (mut editor, clip) = imported();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 200,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 2,
                start: -90,
            },
        })
        .unwrap();
    for block in [1, 7, 64, 512] {
        for (frame, pcm) in render(&editor, 400, block).iter().enumerate() {
            let expected = if (200..240).contains(&frame) {
                let v = value(frame as u64 - 200);
                [v, -v]
            } else {
                [0.0; 2]
            };
            assert_eq!(
                *pcm, expected,
                "hidden negative event, block{block} frame{frame}"
            );
        }
    }
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 110,
        })
        .unwrap();
    assert_eq!(
        (
            editor.project().clip(clip).unwrap().start(),
            editor.project().clip(clip).unwrap().duration()
        ),
        (110, 190)
    );
    for block in [1, 7, 64, 512] {
        for (frame, pcm) in render(&editor, 400, block).iter().enumerate() {
            let source = match frame {
                110..170 => Some(frame as u64 - 110 + 40),
                200..240 => Some(frame as u64 - 200),
                _ => None,
            };
            let expected = source.map_or([0.0; 2], |frame| {
                let v = value(frame);
                [v, -v]
            });
            assert_eq!(
                *pcm, expected,
                "revealed negative event, block{block} frame{frame}"
            );
        }
    }
}

#[test]
fn placed_events_keep_their_original_source_loop_phase_without_repeating_outside_the_gate() {
    let (mut editor, clip) = imported();
    let track = editor.project().tracks()[0].id();
    for command in [
        Command::MoveClip {
            clip,
            track,
            start: 200,
        },
        Command::SetClipLooping {
            clip,
            looping: true,
        },
        Command::SetClipSourcePhase { clip, phase: 65 },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 70 },
        },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 3,
                start: 130,
            },
        },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 1,
                start: -40,
            },
        },
    ] {
        editor.execute(command).unwrap();
    }
    let placed = editor.project().clip(clip).unwrap();
    assert_eq!(
        (
            placed.start(),
            placed.duration(),
            placed.looping(),
            placed.source_phase()
        ),
        (200, 100, true, 65)
    );
    let expected = |output_frame: u64| {
        let v = value((output_frame + 65) % SOURCE);
        [v, -v]
    };
    for block in [1, 7, 64, 512] {
        for (frame, sample) in render(&editor, 400, block).into_iter().enumerate() {
            let expected = match frame as u64 {
                frame @ 240..270 => expected(frame - 200),
                _ => [0.0; 2],
            };
            assert_eq!(sample, expected, "block {block} frame {frame}");
        }
    }
    for (edge, frame) in [(TrimEdge::Left, 160), (TrimEdge::Right, 360)] {
        editor
            .execute(Command::TrimClipEdge { clip, edge, frame })
            .unwrap();
    }
    for block in [1, 7, 64, 512] {
        for (frame, sample) in render(&editor, 400, block).into_iter().enumerate() {
            let expected = match frame as u64 {
                frame @ 160..200 => expected(frame - 160),
                frame @ 240..270 => expected(frame - 200),
                frame @ 330..360 => expected(frame - 260),
                _ => [0.0; 2],
            };
            assert_eq!(sample, expected, "block {block} frame {frame}");
        }
    }
}

#[test]
fn signed_range_placement_gates_moved_pcm_and_preserves_a_hole_when_revealed() {
    for destination in [-80_i64, 120] {
        let (mut editor, clip) = imported();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 200,
            })
            .unwrap();
        for operation in [
            AudioEventEdit::Delete { start: 20, end: 40 },
            AudioEventEdit::PlaceRange {
                start: 10,
                end: 60,
                destination,
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        let start = (200 + destination) as usize;
        for block in [1, 7, 64, 512] {
            let hidden = render(&editor, 400, block);
            assert!(hidden[start..start + 50].iter().all(|f| *f == [0.0; 2]));
        }
        let (edge, frame) = if destination < 0 {
            (TrimEdge::Left, start as u64)
        } else {
            (TrimEdge::Right, (start + 50) as u64)
        };
        editor
            .execute(Command::TrimClipEdge { clip, edge, frame })
            .unwrap();
        for block in [1, 7, 64, 512] {
            for (song, sample) in render(&editor, 400, block).into_iter().enumerate() {
                let source = if (start..start + 10).contains(&song)
                    || (start + 30..start + 50).contains(&song)
                {
                    Some((song - start + 10) as u64)
                } else if (200..210).contains(&song) || (260..300).contains(&song) {
                    Some((song - 200) as u64)
                } else {
                    None
                };
                let expected = source.map_or([0.0; 2], |frame| [value(frame), -value(frame)]);
                assert_eq!(
                    sample, expected,
                    "destination {destination}, block {block}, frame {song}"
                );
            }
        }
    }
}
