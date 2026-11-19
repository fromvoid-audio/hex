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

#[test]
fn split_keeps_outer_age_for_retained_one_shots_and_repeated_placements() {
    for (clip, tempo) in [one_shot(30_000), repeated()] {
        // Cut each ramp, then cut between them. The retained pieces share
        // the original gain at both sides of the cut.
        for elapsed in [1_200, clip.duration() / 2, clip.duration() - 1_800] {
            let cut = clip.start() + elapsed;
            let (left, right) =
                crate::clip_fade_split::split(&clip, cut, ClipId(200), &tempo).unwrap();
            assert_eq!((left.start(), left.duration()), (clip.start(), elapsed));
            assert_eq!(
                (right.start(), right.duration()),
                (cut, clip.duration() - elapsed)
            );
            assert_retained_samples(&clip, &left, &tempo);
            assert_retained_samples(&clip, &right, &tempo);
        }
    }
}

#[test]
fn frame_prefix_rewinds_a_split_and_keeps_silence_before_the_original_fade() {
    let (clip, tempo) = one_shot(30_000);
    let (_, right) = crate::clip_fade_split::split(&clip, 31_200, ClipId(200), &tempo).unwrap();
    let rewound = trim_view(&right, TrimEdge::Left, 28_000, &tempo).unwrap();
    let projection = rewound.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(projection.domain_position_at_frame(29_999).unwrap(), None);
    assert_eq!(projection.gain_at(29_999).unwrap(), 0.0);
    let original = clip.projected_outer_fade(&tempo).unwrap().unwrap();
    for frame in (30_000..30_000 + clip.duration()).step_by(211) {
        assert_eq!(
            projection.domain_position_at_frame(frame).unwrap(),
            original.domain_position_at_frame(frame).unwrap()
        );
        assert_eq!(
            projection.gain_at(frame).unwrap(),
            original.gain_at(frame).unwrap()
        );
    }
}

#[test]
fn direct_musical_lead_and_trim_keep_the_same_exact_authoring_context() {
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
    let domain = Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain);
    domain.fade_projection.as_mut().unwrap().origin_beat_raw += 1;
    let author = domain.fade_projection.as_ref().unwrap().origin_beat_raw;
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();

    let gate = 12_000;
    let direct = clip.with_musical_leading_silence(gate, &tempo).unwrap();
    let expected = clip.projected_outer_fade(&tempo).unwrap().unwrap();
    let extended = direct.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(
        extended.domain_position_at_frame(clip.start() - 1).unwrap(),
        None
    );
    for frame in (clip.start()..clip.start() + clip.duration()).step_by(151) {
        assert_eq!(
            extended.gain_at(frame).unwrap(),
            expected.gain_at(frame).unwrap()
        );
        assert_eq!(
            extended.domain_position_at_frame(frame).unwrap(),
            expected.domain_position_at_frame(frame).unwrap()
        );
    }
    assert_eq!(
        direct
            .audio_edit()
            .unwrap()
            .domain
            .fade_projection
            .as_ref()
            .unwrap()
            .origin_beat_raw,
        author
    );
    assert_eq!(
        direct,
        trim_view(&clip, TrimEdge::Left, gate, &tempo).unwrap()
    );

    let extended_twice = trim_view(&direct, TrimEdge::Left, 6_000, &tempo).unwrap();
    let extended_once = clip.with_musical_leading_silence(6_000, &tempo).unwrap();
    assert_eq!(extended_twice, extended_once);
}
