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
fn frame_trim_rewinds_whole_cycles_then_uses_one_prefix_and_pads_silence_exactly() {
    let (mut original, tempo) = repeated();
    original.start = 50_000;
    crate::audio_edit::project_clip(&mut original, &tempo).unwrap();
    let cropped = trim_view(&original, TrimEdge::Left, 107_522, &tempo).unwrap();
    assert_eq!(cropped.retained_fade().offset(), 57_522);
    let first = trim_view(&original, TrimEdge::Left, 69_174, &tempo).unwrap();
    let successive = trim_view(&first, TrimEdge::Left, 107_522, &tempo).unwrap();
    assert_eq!(successive.retained_fade(), cropped.retained_fade());
    assert_eq!(cropped.audio_edit().unwrap().domain.fade_start, 0);
    let before = original.projected_outer_fade(&tempo).unwrap().unwrap();
    let after = cropped.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in (107_522..126_696).step_by(257) {
        assert_eq!(
            after.gain_at(frame).unwrap(),
            before.gain_at(frame).unwrap()
        );
    }
    let rewound = trim_view(&cropped, TrimEdge::Left, 50_000, &tempo).unwrap();
    assert_eq!(rewound.retained_fade(), original.retained_fade());
    let prefixed = trim_view(&rewound, TrimEdge::Left, 49_000, &tempo).unwrap();
    assert_eq!(prefixed.retained_fade().offset(), 0);
    assert_eq!(prefixed.audio_edit().unwrap().domain.fade_start, 1_000);
    let prefix = prefixed.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(prefix.gain_at(49_999).unwrap(), 0.0);
    assert_eq!(
        prefix.gain_at(50_001).unwrap(),
        before.gain_at(50_001).unwrap()
    );
    let extended = trim_view(&original, TrimEdge::Right, 166_696, &tempo).unwrap();
    assert_eq!(extended.retained_fade().span(), 116_696);
    assert_eq!(extended.retained_fade().out_end_offset(), 40_000);
    let tail = trim_view(&extended, TrimEdge::Left, 150_000, &tempo).unwrap();
    assert_eq!(tail.retained_fade().offset(), 100_000);
    let tail_query = tail.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in [150_000, 160_000, 166_695] {
        assert_eq!(tail_query.gain_at(frame).unwrap(), 0.0);
    }
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
    crate::audio_edit::outer_fade::retain_crop_context(&clip, &mut extended, &tempo).unwrap();
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
    assert_eq!(
        cropped
            .audio_edit()
            .unwrap()
            .domain
            .outer_fade_displacement()
            .unwrap(),
        i128::from(BeatTime::ONE.raw())
    );
    let before = clip.projected_outer_fade(&tempo).unwrap().unwrap();
    let after = cropped.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in (cut..clip.start() + clip.duration()).step_by(997) {
        assert_eq!(
            after.gain_at(frame).unwrap(),
            before.gain_at(frame).unwrap()
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
        crate::audio_edit::outer_fade::retain_crop_context(&clip, &mut missing, &tempo).is_err()
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
