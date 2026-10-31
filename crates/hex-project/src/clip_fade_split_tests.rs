use crate::{AudioEventEdit, Clip, ClipFade, Command, Created, Editor, Project};

fn imported(frames: u64, fade: ClipFade) -> (Editor, crate::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "test".into(),
            path: "test.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    (editor, clip)
}

// Product of the inner event fade (root clock) and the part's own clip fade.
fn gain(part: &Clip, frame: u64) -> f32 {
    let Some(edit) = part.audio_edit() else {
        return part.fade().gain_at(frame);
    };
    let root = edit.view_start + frame;
    let events = part.audio_events();
    let event = events
        .events
        .iter()
        .find(|event| event.start <= root && root < event.end)
        .unwrap();
    let inner = event.fade.map_or(1.0, |fade| {
        fade.shape()
            .gain_at(event.source_start + (root - event.start) - fade.source_start())
    });
    let outer = root
        .checked_sub(edit.domain.fade_start)
        .map_or(0.0, |age| edit.domain.fade.gain_at(age));
    inner * outer
}

fn split(fade: ClipFade, cut: u64) -> (Editor, crate::ClipId, crate::ClipId, Clip) {
    let (mut editor, clip) = imported(fade.span(), fade);
    let original = editor.project().clip(clip).unwrap().clone();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: cut })
        .unwrap()
    else {
        panic!("split")
    };
    (editor, clip, right, original)
}

#[test]
fn a_cut_ramp_moves_onto_the_inner_event_with_unchanged_gain_and_one_step_history() {
    let linear = ClipFade::new(100, 0, 40, 30).unwrap();
    let shaped = linear
        .with_shape(5, 7, 600, -400)
        .unwrap()
        .with_curve_positions(300, 800)
        .unwrap();
    let overlapping = ClipFade::new(100, 0, 70, 60)
        .unwrap()
        .with_shape(0, 0, -300, 200)
        .unwrap();
    // (fade, cut, crossed): inside the fade in, inside the fade out, exactly at
    // either ramp boundary, between ramps, and through overlapping ramps.
    for (fade, cut, crossed) in [
        (linear, 20, true),
        (linear, 85, true),
        (linear, 40, false),
        (linear, 70, false),
        (linear, 55, false),
        (shaped, 3, true),
        (shaped, 30, true),
        (shaped, 45, false),
        (shaped, 95, true),
        (overlapping, 50, true),
    ] {
        let (mut editor, left, right, original) = split(fade, cut);
        let project = editor.project();
        let parts = [project.clip(left).unwrap(), project.clip(right).unwrap()];
        for part in parts {
            let local = part.retained_fade();
            assert_eq!(
                (local.offset(), local.span()),
                (0, part.duration()),
                "no clip fade extends past a part ({fade:?} cut {cut})"
            );
            assert_eq!(part.audio_edit().is_some(), crossed);
            for frame in 0..part.duration() {
                let expected = fade.gain_at(part.start() + frame);
                let actual = gain(part, frame);
                if fade == overlapping {
                    assert!((actual - expected).abs() <= 1e-6, "{actual} {expected}");
                } else {
                    assert_eq!(actual, expected, "cut {cut} frame {}", part.start() + frame);
                }
            }
        }
        // Uncrossed ramps stay ordinary clip fades on the part that holds them.
        let (left_fade, right_fade) = (parts[0].retained_fade(), parts[1].retained_fade());
        if fade == linear && cut == 85 {
            assert_eq!((left_fade.fade_in(), left_fade.fade_out()), (40, 0));
            assert!(right_fade.is_neutral());
        }
        if fade == shaped && cut == 30 {
            assert!(left_fade.is_neutral());
            assert_eq!(
                (
                    right_fade.fade_out(),
                    right_fade.out_end_offset(),
                    right_fade.out_curve(),
                    right_fade.out_position()
                ),
                (30, 7, -400, 800)
            );
        }
        if !crossed {
            assert_eq!(
                (left_fade.fade_in(), right_fade.fade_out()),
                (fade.fade_in(), fade.fade_out())
            );
        }
        assert!(editor.undo());
        assert_eq!(
            editor.project().tracks()[0].clips(),
            std::slice::from_ref(&original)
        );
        assert!(editor.redo());
        assert_eq!(editor.project().tracks()[0].clips().len(), 2);
    }
}

#[test]
fn the_moved_fade_is_an_ordinary_editable_event_fade() {
    let (mut editor, _, right, _) = split(ClipFade::new(100, 0, 40, 30).unwrap(), 20);
    let part = editor.project().clip(right).unwrap();
    let view = part.audio_edit().unwrap().view_start;
    let events = part.editable_audio_events().unwrap();
    let visible = events
        .events
        .iter()
        .find(|event| event.start == view)
        .unwrap();
    assert_eq!(visible.fade.unwrap().shape().fade_in(), 40);
    editor
        .execute(Command::EditAudioEvents {
            clip: right,
            operation: AudioEventEdit::SetFade {
                event: visible.id,
                fade: None,
            },
        })
        .unwrap();
    let part = editor.project().clip(right).unwrap();
    assert!(
        (0..part.duration()).all(|frame| gain(part, frame) == part.retained_fade().gain_at(frame))
    );
    assert!(editor.undo());
}

#[test]
fn looped_and_processed_clips_keep_the_retained_split_law() {
    let (mut editor, clip) = imported(100, ClipFade::new(100, 0, 40, 30).unwrap());
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 20 })
        .unwrap()
    else {
        panic!("split")
    };
    let part = editor.project().clip(right).unwrap();
    assert!(part.audio_edit().is_none());
    assert_eq!((part.fade().offset(), part.fade().span()), (20, 100));

    let (mut editor, clip) = imported(100, ClipFade::new(100, 0, 40, 30).unwrap());
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 60 },
        })
        .unwrap();
    let before = editor.project().clip(clip).unwrap().audio_events();
    editor
        .execute(Command::SplitClip { clip, frame: 20 })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().audio_events(), before);
}
