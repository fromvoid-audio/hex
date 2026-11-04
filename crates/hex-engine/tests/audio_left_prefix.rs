//! Extending a frame source adds silence while its audio stays at song time.
use hex_engine::{Audio, Frame, Renderer, compile};
use hex_project::{
    AudioEventEdit, ClipFade, ClipId, Command, Created, Editor, Project, SourceGainPoint, TrimEdge,
};
use std::collections::BTreeMap;

fn fixture() -> (Editor, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "prefix".into(),
            path: "prefix.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("import")
    };
    e.execute(Command::MoveClip {
        clip,
        track,
        start: 4000,
    })
    .unwrap();
    e.execute(Command::SetClipFadeEnvelope {
        clip,
        fade: ClipFade::new(128, 0, 13, 17).unwrap(),
    })
    .unwrap();
    e.execute(Command::SetAudioClipGain {
        clip,
        millidb: -6000,
    })
    .unwrap();
    (e, clip)
}
fn render(e: &Editor, block: usize) -> Vec<Frame> {
    let asset = e.project().assets().next().unwrap().id();
    let audio = Audio::new(
        48_000,
        (0..128)
            .map(|i| {
                let value = (i as f32 + 1.0) / 512.0;
                [value, -value / 2.0]
            })
            .collect(),
    )
    .unwrap();
    let graph = compile::project(e.project(), &BTreeMap::from([(asset, audio)])).unwrap();
    let mut renderer = Renderer::new(&graph);
    renderer.play();
    let mut out = vec![[0.0; 2]; 4500];
    for chunk in out.chunks_mut(block) {
        renderer.render(chunk).unwrap();
    }
    out
}
#[test]
fn prefix_preserves_plain_and_edited_pcm_and_rewinds_available_media_first() {
    for case in 0..7 {
        let (mut e, clip) = fixture();
        if case == 1 {
            e.execute(Command::TrimClip {
                clip,
                start: 4000,
                duration: 64,
                source_offset: 32,
                source_length: 64,
            })
            .unwrap();
            // Existing source rewind remains the reference before padding.
            e.execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 3968,
            })
            .unwrap();
        } else if case == 2 {
            e.execute(Command::SetAudioClipSourceGain {
                clip,
                points: vec![
                    SourceGainPoint {
                        frame: 0,
                        gain_q16: 32768,
                    },
                    SourceGainPoint {
                        frame: 128,
                        gain_q16: 65536,
                    },
                ],
            })
            .unwrap();
            e.execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 64 },
            })
            .unwrap();
            e.execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 2,
                    fade: Some(
                        hex_project::AudioEventFade::new(64, ClipFade::new(64, 0, 7, 11).unwrap())
                            .unwrap(),
                    ),
                },
            })
            .unwrap();
            e.execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Move {
                    event: 2,
                    start: 48,
                },
            })
            .unwrap();
            e.execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 4020,
            })
            .unwrap();
        }
        if case == 6 {
            e.execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 64 },
            })
            .unwrap();
            e.execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Place {
                    event: 2,
                    start: 160,
                },
            })
            .unwrap();
            assert!(e.project().clip(clip).unwrap().has_retained_audio_roots());
        }
        if (3..6).contains(&case) {
            let cents = [137, -4800, 4800][case - 3];
            e.execute(Command::SetAudioClipPitch { clip, cents })
                .unwrap();
        }
        let old = e.project().clip(clip).unwrap();
        let compare_from = if case == 2 { old.start() as usize } else { 0 };
        let source_origin = old.start() - old.audio_edit().map_or(0, |edit| edit.view_start);
        let original = render(&e, 4500);
        e.execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 3500,
        })
        .unwrap();
        for block in [1, 7, 64, 512] {
            let actual = render(&e, block);
            let max = actual[compare_from..]
                .iter()
                .flatten()
                .zip(original[compare_from..].iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            // The plain renderer folds clip gain/fade into its read; a prepared
            // event root applies the same f32 factors in two stages.
            assert!(max <= 1e-7, "case {case} block {block}: {max}");
            assert!(
                actual[3500..source_origin as usize]
                    .iter()
                    .all(|f| *f == [0.0; 2])
            );
        }
    }
}
