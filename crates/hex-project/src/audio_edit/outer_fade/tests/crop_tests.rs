use super::*;
use crate::audio_edit::{TrimEdge, trim_view};

#[test]
fn signed_destination_inverse_retains_frame_endpoints_and_unsigned_refusal() {
    let (mut clip, tempo) = repeated();
    clip.start = 50_000;
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    let view = AudioViewProjection::new(&clip, &tempo).unwrap();
    for (frame, expected) in [
        (0, -50_000),
        (49_983, -17),
        (50_000, 0),
        (50_013, 13),
        (126_696, 76_696),
        (126_713, 76_713),
    ] {
        assert_eq!(view.signed_elapsed_at_frame(frame).unwrap(), expected);
        if expected >= 0 {
            assert_eq!(view.elapsed_at_frame(frame).unwrap(), expected as u64);
        } else {
            assert_eq!(
                view.elapsed_at_frame(frame).unwrap_err().0,
                "audio content frame precedes placement"
            );
        }
    }
}

#[test]
fn frame_trim_reanchors_outer_fade_to_corners_across_crops_and_extensions() {
    let (mut original, tempo) = repeated();
    original.start = 50_000;
    crate::audio_edit::project_clip(&mut original, &tempo).unwrap();
    let cropped = trim_view(&original, TrimEdge::Left, 107_522, &tempo).unwrap();
    let fade = cropped.retained_fade();
    assert_eq!(
        (
            fade.offset(),
            fade.span(),
            fade.in_start(),
            fade.out_end_offset()
        ),
        (0, cropped.duration(), 0, 0)
    );
    assert_eq!(cropped.audio_edit().unwrap().domain.fade_start, 0);
    let first = trim_view(&original, TrimEdge::Left, 69_174, &tempo).unwrap();
    let successive = trim_view(&first, TrimEdge::Left, 107_522, &tempo).unwrap();
    assert_eq!(successive.retained_fade(), cropped.retained_fade());
    let after = cropped.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in (107_522..126_696).step_by(257) {
        assert_eq!(
            after.gain_at(frame).unwrap(),
            fade.gain_at(frame - cropped.start())
        );
    }
    let rewound = trim_view(&cropped, TrimEdge::Left, 50_000, &tempo).unwrap();
    assert_eq!(
        (
            rewound.retained_fade().offset(),
            rewound.retained_fade().span(),
            rewound.retained_fade().in_start(),
            rewound.retained_fade().out_end_offset()
        ),
        (0, rewound.duration(), 0, 0)
    );
    let prefixed = trim_view(&rewound, TrimEdge::Left, 49_000, &tempo).unwrap();
    assert_eq!(prefixed.retained_fade().offset(), 0);
    assert_eq!(prefixed.retained_fade().span(), prefixed.duration());
    assert_eq!(prefixed.audio_edit().unwrap().domain.fade_start, 0);
    let extended = trim_view(&original, TrimEdge::Right, 166_696, &tempo).unwrap();
    assert_eq!(
        (
            extended.retained_fade().offset(),
            extended.retained_fade().span(),
            extended.retained_fade().in_start(),
            extended.retained_fade().out_end_offset()
        ),
        (0, extended.duration(), 0, 0)
    );
    let tail = trim_view(&extended, TrimEdge::Left, 150_000, &tempo).unwrap();
    assert_eq!(
        (
            tail.retained_fade().offset(),
            tail.retained_fade().span(),
            tail.retained_fade().in_start(),
            tail.retained_fade().out_end_offset()
        ),
        (0, tail.duration(), 0, 0)
    );
}

#[test]
fn neutral_padding_stays_unity_and_repeat_context_application_is_idempotent() {
    let (mut clip, tempo) = repeated();
    Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain).fade =
        ClipFade::neutral(76_696).unwrap();
    let mut extended = trim_view(&clip, TrimEdge::Right, 120_000, &tempo).unwrap();
    assert!(extended.retained_fade().is_neutral());
    assert_eq!(extended.retained_fade().out_end_offset(), 0);
    assert_eq!(
        extended
            .projected_outer_fade(&tempo)
            .unwrap()
            .unwrap()
            .gain_at(119_999)
            .unwrap(),
        1.0
    );
    let once = extended.clone();
    crate::audio_edit::retain_outer_fade_crop_context(&clip, &mut extended, &tempo).unwrap();
    assert_eq!(extended, once);
}

#[test]
fn signed_frozen_directed_clock_and_crop_keep_forward_gain_once() {
    let (mut clip, tempo) = repeated();
    clip.audio_edit_mut()
        .unwrap()
        .anchor_at_reference(0, 0, &tempo)
        .unwrap();
    let edit = clip.audio_edit_mut().unwrap();
    edit.timing = AudioClipTiming::Frozen {
        reference_start_raw: 0,
        tempo: Arc::new(tempo.clone()),
    };
    let length = edit.placement.unwrap().outer_length.length();
    edit.content_view = Some(AudioContentView {
        restore_plain: false,
        base_view_start: edit.view_start,
        base_view_end: edit.view_end,
        base_placement: edit.placement,
        base_start_delta: 0,
        window: ContentWindow::new(0, length, true).unwrap(),
        read_resampler: crate::WarpResampler::default(),
    });
    clip.start = 300_000;
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    let view = AudioViewProjection::new(&clip, &tempo).unwrap();
    assert_eq!(
        view.signed_elapsed_at_frame(276_000).unwrap(),
        -i128::from(BeatTime::ONE.raw())
    );
    assert_eq!(view.signed_elapsed_at_frame(300_000).unwrap(), 0);
    assert_eq!(
        view.signed_elapsed_at_frame(300_000 + clip.duration())
            .unwrap(),
        i128::from(length)
    );
    assert!(
        view.signed_elapsed_at_frame(300_000 + clip.duration() + 24_000)
            .unwrap()
            > i128::from(length)
    );
    let cut = 324_000;
    let cropped = trim_view(&clip, TrimEdge::Left, cut, &tempo).unwrap();
    let (left, right) =
        crate::clip_fade_split::split(&clip, cut, crate::ClipId(200), &tempo).unwrap();
    for part in [&cropped, &left, &right] {
        let fade = part.retained_fade();
        assert_eq!(
            (
                fade.offset(),
                fade.span(),
                fade.in_start(),
                fade.out_end_offset()
            ),
            (0, part.duration(), 0, 0)
        );
        assert_eq!(
            part.audio_edit()
                .unwrap()
                .domain
                .outer_fade_displacement()
                .unwrap(),
            0
        );
    }
    assert_eq!(
        (
            left.retained_fade().fade_in(),
            left.retained_fade().fade_out()
        ),
        (clip.retained_fade().fade_in().min(left.duration()), 0)
    );
    assert_eq!(
        (
            right.retained_fade().fade_in(),
            right.retained_fade().fade_out()
        ),
        (0, clip.retained_fade().fade_out().min(right.duration()))
    );
    let after = cropped.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in (cut..clip.start() + clip.duration()).step_by(997) {
        assert_eq!(
            after.gain_at(frame).unwrap(),
            cropped.retained_fade().gain_at(frame - cropped.start())
        );
    }
}

#[test]
fn context_refusal_preserves_result_and_legacy_trim_path_is_unchanged() {
    let (clip, tempo) = repeated();
    let mut missing = clip.clone();
    missing.edit = None;
    let before = missing.clone();
    assert!(
        crate::audio_edit::retain_outer_fade_crop_context(&clip, &mut missing, &tempo).is_err()
    );
    assert_eq!(missing, before);
    let mut legacy = clip.clone();
    Arc::make_mut(&mut legacy.audio_edit_mut().unwrap().domain).fade_clock =
        AudioClipFadeClock::LegacyRoot;
    let trimmed = trim_view(&legacy, TrimEdge::Left, 19_174, &tempo).unwrap();
    assert_eq!(
        trimmed.audio_edit().unwrap().domain.fade_clock,
        AudioClipFadeClock::LegacyRoot
    );
    assert_eq!(trimmed.retained_fade(), legacy.retained_fade());
}
