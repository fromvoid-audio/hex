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
fn split_anchors_every_part_fade_to_its_corners_with_one_step_history() {
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
    // Inside fade-in, inside fade-out, at ramp boundaries, between ramps, and
    // through overlapping ramps: every cut keeps both halves corner-anchored.
    for (fade, cut) in [
        (linear, 20),
        (linear, 85),
        (linear, 40),
        (linear, 70),
        (linear, 55),
        (shaped, 3),
        (shaped, 30),
        (shaped, 45),
        (shaped, 95),
        (overlapping, 50),
    ] {
        let (mut editor, left, right, original) = split(fade, cut);
        let project = editor.project();
        let parts = [project.clip(left).unwrap(), project.clip(right).unwrap()];
        for part in parts {
            let local = part.retained_fade();
            assert_eq!(
                (
                    local.offset(),
                    local.span(),
                    local.in_start(),
                    local.out_end_offset()
                ),
                (0, part.duration(), 0, 0),
                "split must anchor fade to part corners ({fade:?} cut {cut})"
            );
            assert!(local.fade_in() + local.fade_out() <= part.duration());
            assert!(part.audio_edit().is_none());
            for frame in 0..part.duration() {
                assert_eq!(gain(part, frame), local.gain_at(frame));
            }
        }
        let (left_fade, right_fade) = (parts[0].retained_fade(), parts[1].retained_fade());
        assert_eq!(left_fade.fade_in(), fade.fade_in().min(parts[0].duration()));
        assert_eq!(left_fade.fade_out(), 0);
        assert_eq!(
            (left_fade.in_curve(), left_fade.in_position()),
            (fade.in_curve(), fade.in_position())
        );
        assert_eq!(right_fade.fade_in(), 0);
        assert_eq!(
            right_fade.fade_out(),
            fade.fade_out().min(parts[1].duration())
        );
        assert_eq!(
            (right_fade.out_curve(), right_fade.out_position()),
            (fade.out_curve(), fade.out_position())
        );
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
fn repeated_cuts_on_faded_clips_stay_corner_anchored_without_inventing_inner_event_fades() {
    let (mut editor, left, right, _) = split(ClipFade::new(100, 0, 40, 30).unwrap(), 20);
    let Created::Clip(right_tail) = editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 85,
        })
        .unwrap()
    else {
        panic!("second split")
    };
    let project = editor.project();
    let left_clip = project.clip(left).unwrap();
    let middle_clip = project.clip(right).unwrap();
    let tail_clip = project.clip(right_tail).unwrap();
    for (part, expected_in, expected_out) in
        [(left_clip, 20, 0), (middle_clip, 0, 0), (tail_clip, 0, 15)]
    {
        let fade = part.retained_fade();
        assert_eq!(
            (
                fade.offset(),
                fade.span(),
                fade.in_start(),
                fade.out_end_offset(),
                fade.fade_in(),
                fade.fade_out()
            ),
            (0, part.duration(), 0, 0, expected_in, expected_out)
        );
        assert!(
            part.audio_events()
                .events
                .iter()
                .all(|event| event.fade.is_none())
        );
    }
    assert!(editor.undo());
    assert!(editor.undo());
}

#[test]
fn looped_and_event_edited_clips_reanchor_fades_at_corners_on_split() {
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
    let left_part = editor.project().clip(clip).unwrap();
    let right_part = editor.project().clip(right).unwrap();
    assert!(left_part.audio_edit().is_none());
    assert!(right_part.audio_edit().is_none());
    assert_eq!(
        (
            left_part.fade().offset(),
            left_part.fade().span(),
            left_part.fade().fade_in(),
            left_part.fade().fade_out()
        ),
        (0, 20, 20, 0)
    );
    assert_eq!(
        (
            right_part.fade().offset(),
            right_part.fade().span(),
            right_part.fade().fade_in(),
            right_part.fade().fade_out()
        ),
        (0, 80, 0, 30)
    );

    let (mut editor, clip) = imported(100, ClipFade::new(100, 0, 40, 30).unwrap());
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 60 },
        })
        .unwrap();
    let before = editor.project().clip(clip).unwrap().audio_events();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 20 })
        .unwrap()
    else {
        panic!("split")
    };
    assert_eq!(editor.project().clip(clip).unwrap().audio_events(), before);
    let left_fade = editor.project().clip(clip).unwrap().retained_fade();
    let right_fade = editor.project().clip(right).unwrap().retained_fade();
    assert_eq!(
        (
            left_fade.offset(),
            left_fade.span(),
            left_fade.in_start(),
            left_fade.out_end_offset(),
            left_fade.fade_in(),
            left_fade.fade_out()
        ),
        (0, 20, 0, 0, 20, 0)
    );
    assert_eq!(
        (
            right_fade.offset(),
            right_fade.span(),
            right_fade.in_start(),
            right_fade.out_end_offset(),
            right_fade.fade_in(),
            right_fade.fade_out()
        ),
        (0, 80, 0, 0, 0, 30)
    );
}
