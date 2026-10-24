//! Actual PCM acceptance of the canonical finite placement law.
use super::*;
use hex_project::{AudioViewMode, AudioViewPlacement, ClipFade};

fn placed(
    mode: AudioViewMode,
    phase: u64,
) -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 1 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 0 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = row.edit.as_mut().unwrap();
    edit.view_start = 1_000;
    edit.view_end = 3_000;
    edit.placement = Some(AudioViewPlacement {
        mode,
        outer_length: AudioDomainClock::Frames(5_200),
        cycle_phase: phase,
    });
    row.duration = 5_200;
    row.fade = ClipFade::neutral(row.duration).unwrap();
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap(),
        clip,
        media,
    )
}

#[test]
fn finite_audio_placement_renders_repeat_phase_partial_cycle_and_once_silence() {
    for (mode, phase) in [
        (AudioViewMode::Repeat, 500),
        (AudioViewMode::Once, 500),
        (AudioViewMode::Once, 3_000),
    ] {
        let (editor, clip, media) = placed(mode, phase);
        let before = editor.project().snapshot();
        let source = media.values().next().unwrap().frames();
        let expected = (0..5_200)
            .map(|elapsed| {
                let clock = phase + elapsed;
                if mode == AudioViewMode::Repeat || clock < 2_000 {
                    source[(1_000 + clock % 2_000) as usize]
                } else {
                    [0.0; 2]
                }
            })
            .collect::<Vec<_>>();
        let context = AudioTransformPreparation::default();
        let prepared = prepare(&editor, &media, &context);
        for block in [1, 127, 257] {
            let actual = render(&prepared, 0, expected.len(), block);
            let first_mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
            assert_eq!(
                first_mismatch, None,
                "mode={mode:?} phase={phase} block={block}"
            );
            assert_eq!(render(&prepared, 1_497, 701, block), expected[1_497..2_198]);
        }
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.project().clip(clip).is_some());
    }
}
