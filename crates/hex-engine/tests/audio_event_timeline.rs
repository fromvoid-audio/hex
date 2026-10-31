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
