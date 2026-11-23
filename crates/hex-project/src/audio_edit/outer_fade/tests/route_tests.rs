use super::*;
use crate::{ClipId, TrimEdge, audio_edit::trim_view};

fn one_shot(start: u64) -> (Clip, TempoMap) {
    let (mut clip, tempo) = repeated();
    clip.start = start;
    let edit = clip.audio_edit_mut().unwrap();
    edit.placement = None;
    let fade = ClipFade::new(edit.view_end, 0, 2_400, 3_600).unwrap();
    Arc::make_mut(&mut edit.domain).fade = fade;
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    (clip, tempo)
}

fn assert_retained_samples(original: &Clip, retained: &Clip, tempo: &TempoMap) {
    let original = original.projected_outer_fade(tempo).unwrap().unwrap();
    let fade = retained.projected_outer_fade(tempo).unwrap().unwrap();
    let start = retained.start();
    let end = start + retained.duration();
    for frame in (start..end).step_by(173).chain([end - 1]) {
        assert_eq!(
            fade.domain_position_at_frame(frame).unwrap(),
            original.domain_position_at_frame(frame).unwrap(),
            "frame {frame}"
        );
        assert_eq!(
            fade.gain_at(frame).unwrap(),
            original.gain_at(frame).unwrap(),
            "frame {frame}"
        );
    }
}

fn assert_corner_anchored_outer_fade(retained: &Clip, tempo: &TempoMap) {
    let fade = retained.retained_fade();
    assert_eq!(
        (
            fade.offset(),
            fade.span(),
            fade.in_start(),
            fade.out_end_offset()
        ),
        (0, retained.duration(), 0, 0)
    );
    assert!(fade.fade_in() + fade.fade_out() <= retained.duration());
    let projected = retained.projected_outer_fade(tempo).unwrap().unwrap();
    let start = retained.start();
    let end = start + retained.duration();
    for frame in (start..end).step_by(173).chain([end - 1]) {
        assert_eq!(
            projected.domain_position_at_frame(frame).unwrap(),
            Some(frame - start),
            "frame {frame}"
        );
        assert_eq!(
            projected.gain_at(frame).unwrap(),
            fade.gain_at(frame - start),
            "frame {frame}"
        );
    }
}

#[test]
fn split_reanchors_outer_fade_at_corners_for_retained_one_shots_and_repeated_placements() {
    for (clip, tempo) in [one_shot(30_000), repeated()] {
        let original_fade = clip.retained_fade();
        for elapsed in [1_200, clip.duration() / 2, clip.duration() - 1_800] {
            let cut = clip.start() + elapsed;
            let (left, right) =
                crate::clip_fade_split::split(&clip, cut, ClipId(200), &tempo).unwrap();
            assert_eq!((left.start(), left.duration()), (clip.start(), elapsed));
            assert_eq!(
                (right.start(), right.duration()),
                (cut, clip.duration() - elapsed)
            );
            assert_eq!(
                (
                    left.retained_fade().fade_in(),
                    left.retained_fade().fade_out()
                ),
                (original_fade.fade_in().min(left.duration()), 0)
            );
            assert_eq!(
                (
                    right.retained_fade().fade_in(),
                    right.retained_fade().fade_out()
                ),
                (0, original_fade.fade_out().min(right.duration()))
            );
            assert_corner_anchored_outer_fade(&left, &tempo);
            assert_corner_anchored_outer_fade(&right, &tempo);
        }
    }
}

#[test]
fn frame_prefix_rewinds_a_split_with_corner_anchored_outer_fade() {
    let (clip, tempo) = one_shot(30_000);
    let (_, right) = crate::clip_fade_split::split(&clip, 31_200, ClipId(200), &tempo).unwrap();
    let rewound = trim_view(&right, TrimEdge::Left, 28_000, &tempo).unwrap();
    assert_corner_anchored_outer_fade(&rewound, &tempo);
    assert_eq!(
        (
            rewound.retained_fade().fade_in(),
            rewound.retained_fade().fade_out()
        ),
        (0, clip.retained_fade().fade_out())
    );
}

#[test]
fn direct_musical_lead_and_trim_keep_corner_anchored_outer_fade_consistent() {
    let tempo = TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(BeatTime::ONE, Tempo::new(60_000_000).unwrap()),
        ],
    )
    .unwrap();
    let (mut clip, _) = one_shot(24_000);
    clip.audio_edit_mut()
        .unwrap()
        .anchor_at_reference(24_000, 24_000, &tempo)
        .unwrap();
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();

    let gate = 12_000;
    let direct = clip.with_musical_leading_silence(gate, &tempo).unwrap();
    assert_eq!(
        direct,
        trim_view(&clip, TrimEdge::Left, gate, &tempo).unwrap()
    );
    let fade = direct.retained_fade();
    assert_eq!(
        (
            fade.offset(),
            fade.span(),
            fade.in_start(),
            fade.out_end_offset()
        ),
        (0, direct.duration(), 0, 0)
    );
    assert_eq!(
        direct
            .audio_edit()
            .unwrap()
            .domain
            .outer_fade_displacement()
            .unwrap(),
        0
    );

    let extended_twice = trim_view(&direct, TrimEdge::Left, 6_000, &tempo).unwrap();
    let extended_once = clip.with_musical_leading_silence(6_000, &tempo).unwrap();
    assert_eq!(extended_twice, extended_once);
}

#[test]
fn frame_prefix_preserves_destination_age_when_anchored_or_source_retimed() {
    let (mut clip, tempo) = one_shot(30_000);
    Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain).fade_start = 600;
    let before = clip.clone();
    clip.audio_edit_mut()
        .unwrap()
        .anchor(30_000, &tempo)
        .unwrap();
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    assert_retained_samples(&before, &clip, &tempo);
    let author = clip.audio_edit().unwrap().domain.fade_projection.clone();
    let stretched = crate::preview_audio_clip_stretch(
        &clip,
        TrimEdge::Right,
        clip.start() + clip.duration() * 2,
        &tempo,
        19_174,
    )
    .unwrap();
    assert!(stretched.duration() > clip.duration());
    assert_eq!(stretched.retained_fade(), clip.retained_fade());
    assert_eq!(
        stretched.audio_edit().unwrap().domain.fade_projection,
        author
    );
    assert_retained_samples(&stretched, &clip, &tempo);
}
