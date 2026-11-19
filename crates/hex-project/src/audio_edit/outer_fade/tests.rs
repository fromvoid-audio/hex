use super::*;
use crate::{
    AudioClipEdit, AudioClipTiming, AudioContentView, AudioViewMode, AudioViewPlacement, BeatTime,
    Command, ContentWindow, Created, Editor, Project, Tempo, TempoPoint,
};
use std::sync::Arc;

fn repeated() -> (Clip, TempoMap) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "outer".into(),
            path: "outer.wav".into(),
            sample_rate: 48_000,
            frames: 19_174,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    let mut clip = editor.project().clip(clip).unwrap().clone();
    let mut edit = AudioClipEdit::capture(&clip);
    edit.placement = Some(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Frames(76_696),
        cycle_phase: 0,
    });
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade = ClipFade::new(76_696, 0, 2_400, 3_600).unwrap();
    clip.edit = Some(Arc::new(edit));
    let tempo = editor.project().tempo_map().clone();
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    (clip, tempo)
}

#[test]
fn reported_four_cycle_geometry_has_one_forward_envelope_and_exact_knots() {
    let (clip, tempo) = repeated();
    let fade = clip.retained_fade();
    let projection = clip.projected_outer_fade(&tempo).unwrap().unwrap();
    for elapsed in [
        0, 1, 1_200, 2_399, 19_173, 19_174, 38_348, 57_522, 73_096, 76_695,
    ] {
        assert_eq!(
            projection.gain_at(clip.start() + elapsed).unwrap(),
            fade.gain_at(elapsed)
        );
    }
    for seam in [19_174, 38_348, 57_522] {
        assert_eq!(projection.gain_at(seam - 1).unwrap(), 1.0);
        assert_eq!(projection.gain_at(seam).unwrap(), 1.0);
        assert_eq!(projection.gain_at(seam + 1).unwrap(), 1.0);
    }
    for position in fade.breakpoints().into_iter().flatten() {
        let frame = projection
            .frame_at_domain_position(position)
            .unwrap()
            .unwrap();
        assert_eq!(
            projection.domain_position_at_frame(frame).unwrap(),
            Some(position)
        );
    }
    assert_eq!(
        projection.domain_position_at_frame(76_696).unwrap(),
        Some(76_696)
    );
    assert!(projection.gain_at(76_696).is_err());
    assert!(projection.domain_position_at_frame(76_697).is_err());
    assert!(projection.frame_at_domain_position(76_697).is_err());
}

#[test]
fn full_offset_or_prefix_context_does_not_wrap_and_compound_state_is_refused() {
    let (mut offset_clip, tempo) = repeated();
    Arc::make_mut(&mut offset_clip.audio_edit_mut().unwrap().domain).fade =
        ClipFade::new(160_000, 50_000, 80_000, 4_000).unwrap();
    let offset = offset_clip.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(offset.domain_position_at_frame(0).unwrap(), Some(50_000));
    assert_eq!(
        offset.domain_position_at_frame(57_522).unwrap(),
        Some(107_522)
    );
    assert_eq!(offset.frame_at_domain_position(49_999).unwrap(), None);
    assert_eq!(offset.frame_at_domain_position(159_999).unwrap(), None);
    assert_eq!(
        offset.gain_at(57_522).unwrap(),
        offset_clip.retained_fade().gain_at(57_522)
    );
    assert!(
        offset
            .breakpoints()
            .unwrap()
            .into_iter()
            .flatten()
            .all(|frame| frame <= offset_clip.duration())
    );
    let (mut prefix_clip, _) = repeated();
    let domain = Arc::make_mut(&mut prefix_clip.audio_edit_mut().unwrap().domain);
    domain.fade = ClipFade::new(160_000, 0, 80_000, 4_000).unwrap();
    domain.fade_start = 13;
    let prefix = prefix_clip.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(prefix.gain_at(0).unwrap(), 0.0);
    assert_eq!(prefix.domain_position_at_frame(12).unwrap(), None);
    assert_eq!(prefix.domain_position_at_frame(13).unwrap(), Some(0));
    assert_eq!(
        prefix.domain_position_at_frame(57_522).unwrap(),
        Some(57_509)
    );
    let domain = Arc::make_mut(&mut offset_clip.audio_edit_mut().unwrap().domain);
    domain.fade_start = 13;
    assert!(
        offset_clip.projected_outer_fade(&tempo).is_err(),
        "compound offset/prefix has an extra hard gate"
    );
    Arc::make_mut(&mut offset_clip.audio_edit_mut().unwrap().domain).fade_clock =
        AudioClipFadeClock::LegacyRoot;
    assert!(
        offset_clip.projected_outer_fade(&tempo).unwrap().is_none(),
        "historical law is unchanged"
    );
}

#[test]
fn musical_frozen_move_and_directed_source_reverse_keep_destination_envelope() {
    let (mut clip, authored) = repeated();
    let mut edit = clip.audio_edit().unwrap().clone();
    // Build the existing musical placement without admitting the new clock.
    edit.anchor_at_reference(0, 0, &authored).unwrap();
    clip.edit = Some(Arc::new(edit));
    let current = TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(
                BeatTime::from_quarters(2).unwrap(),
                Tempo::new(60_000_000).unwrap(),
            ),
        ],
    )
    .unwrap();
    crate::audio_edit::project_clip(&mut clip, &current).unwrap();
    let forward = clip.projected_outer_fade(&current).unwrap().unwrap();
    for frame in (0..clip.duration()).step_by(997) {
        let beat = current.frame_to_beat(frame).unwrap();
        let age = authored.beat_to_frame(beat).unwrap();
        assert_eq!(
            forward.gain_at(frame).unwrap(),
            clip.retained_fade().gain_at(age)
        );
    }
    for position in clip.retained_fade().breakpoints().into_iter().flatten() {
        if let Some(frame) = forward.frame_at_domain_position(position).unwrap() {
            assert!(
                forward
                    .domain_position_at_frame(frame)
                    .unwrap()
                    .unwrap()
                    .abs_diff(position)
                    <= 1
            );
        }
    }
    let mut frozen = clip.clone();
    let edit = frozen.audio_edit_mut().unwrap();
    edit.timing = AudioClipTiming::Frozen {
        reference_start_raw: 0,
        tempo: Arc::new(current.clone()),
    };
    edit.processing_projection = None;
    frozen.start = 300_000;
    crate::audio_edit::project_clip(&mut frozen, &authored).unwrap();
    let moved = frozen.projected_outer_fade(&authored).unwrap().unwrap();
    assert_eq!(frozen.duration(), clip.duration());
    for elapsed in (0..clip.duration()).step_by(997) {
        assert_eq!(
            moved.gain_at(frozen.start() + elapsed).unwrap(),
            forward.gain_at(elapsed).unwrap()
        );
    }
    let mut reversed = frozen.clone();
    let edit = reversed.audio_edit_mut().unwrap();
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
    crate::audio_edit::project_clip(&mut reversed, &authored).unwrap();
    let reversed_fade = reversed.projected_outer_fade(&authored).unwrap().unwrap();
    for elapsed in (0..clip.duration()).step_by(997) {
        assert_eq!(
            reversed_fade.gain_at(reversed.start() + elapsed).unwrap(),
            moved.gain_at(frozen.start() + elapsed).unwrap()
        );
    }
}

#[test]
fn legacy_and_malformed_author_context_are_distinct_from_outer_capability() {
    let (mut clip, tempo) = repeated();
    let domain = Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain);
    domain.fade_clock = AudioClipFadeClock::LegacyRoot;
    assert!(clip.projected_outer_fade(&tempo).unwrap().is_none());
    let domain = Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain);
    domain.fade_clock = AudioClipFadeClock::Outer;
    domain.fade_projection = Some(AudioFadeProjection {
        tempo: tempo.clone(),
        origin_beat_raw: 0,
        local_start_raw: 0,
    });
    assert!(
        clip.projected_outer_fade(&tempo).is_err(),
        "frame domain cannot pretend author beats are frames"
    );
    assert_eq!(
        clip.audio_edit()
            .unwrap()
            .validate(&clip, &tempo, 19_174)
            .unwrap_err()
            .0,
        "outer clip fades are not supported yet"
    );
}

#[test]
fn held_legacy_override_and_acknowledged_outer_share_capture_law() {
    for musical in [false, true] {
        let (mut clip, tempo) = repeated();
        if musical {
            clip.audio_edit_mut()
                .unwrap()
                .anchor_at_reference(0, 0, &tempo)
                .unwrap();
            crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
        }
        Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain).fade_clock =
            AudioClipFadeClock::LegacyRoot;
        let fade = ClipFade::new(clip.duration(), 0, 1_200, 2_400).unwrap();
        let held = clip.projected_outer_fade_preview(&tempo, fade).unwrap();
        let mut accepted = clip.clone();
        let placement = AudioViewProjection::new(&accepted, &tempo).unwrap();
        let author = musical.then(|| AudioFadeProjection {
            tempo: placement.placement_tempo().clone(),
            origin_beat_raw: placement.placement_reference(),
            local_start_raw: 0,
        });
        let domain = Arc::make_mut(&mut accepted.audio_edit_mut().unwrap().domain);
        domain.fade_clock = AudioClipFadeClock::Outer;
        domain.fade = fade;
        domain.fade_start = 0;
        domain.fade_projection = author;
        let canonical = accepted.projected_outer_fade(&tempo).unwrap().unwrap();
        assert_eq!(
            held.breakpoints().unwrap(),
            canonical.breakpoints().unwrap()
        );
        for elapsed in (0..clip.duration()).step_by(997) {
            assert_eq!(
                held.gain_at(elapsed).unwrap(),
                canonical.gain_at(elapsed).unwrap()
            );
            assert_eq!(
                held.domain_position_at_frame(elapsed).unwrap(),
                canonical.domain_position_at_frame(elapsed).unwrap()
            );
        }
        assert!(
            clip.projected_outer_fade_preview(
                &tempo,
                ClipFade::neutral(clip.duration() - 1).unwrap()
            )
            .is_err()
        );
        assert!(
            clip.projected_outer_fade_preview(
                &tempo,
                ClipFade::new(clip.duration(), 1, 0, 0).unwrap()
            )
            .is_err()
        );
    }
}

#[test]
fn zero_author_age_keeps_subframe_positive_and_negative_origin_identity() {
    let (mut clip, tempo) = repeated();
    clip.audio_edit_mut()
        .unwrap()
        .anchor_at_reference(0, 0, &tempo)
        .unwrap();
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    for origin in [1_i128, -i128::from(BeatTime::ONE.raw()) + 1] {
        assert_ne!(
            audio_projection_beat(&tempo, audio_projection_frame(&tempo, origin).unwrap()).unwrap(),
            origin,
            "fixture origin must not align with a frame"
        );
        for pre_context in [0, BeatTime::ONE.raw()] {
            let domain = Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain);
            domain.fade_start = pre_context;
            domain.fade_projection = Some(AudioFadeProjection {
                tempo: tempo.clone(),
                origin_beat_raw: origin,
                local_start_raw: 0,
            });
            let expected = AudioViewProjection::new(&clip, &tempo)
                .unwrap()
                .frame_at_elapsed(pre_context)
                .unwrap();
            let projection = clip.projected_outer_fade(&tempo).unwrap().unwrap();
            assert_eq!(
                projection
                    .frame_at_domain_position(clip.retained_fade().offset())
                    .unwrap(),
                Some(expected)
            );
            assert_eq!(
                projection.domain_position_at_frame(expected).unwrap(),
                Some(clip.retained_fade().offset())
            );
        }
    }
}

#[test]
fn cropped_musical_context_retains_immutable_author_origin_across_tempo_seam() {
    let author = TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(BeatTime::ONE, Tempo::new(60_000_000).unwrap()),
        ],
    )
    .unwrap();
    for origin in [0_i128, -i128::from(BeatTime::ONE.raw()) + 1] {
        let (mut original, _) = repeated();
        original
            .audio_edit_mut()
            .unwrap()
            .anchor_at_reference(0, 0, &author)
            .unwrap();
        let domain = Arc::make_mut(&mut original.audio_edit_mut().unwrap().domain);
        domain.fade_projection.as_mut().unwrap().origin_beat_raw = origin;
        domain.fade = ClipFade::new(76_696, 0, 60_000, 60_000).unwrap();
        crate::audio_edit::project_clip(&mut original, &author).unwrap();
        let cut = author
            .beat_to_frame(BeatTime::from_quarters(2).unwrap())
            .unwrap();
        let consumed = AudioViewProjection::new(&original, &author)
            .unwrap()
            .elapsed_at_frame(cut)
            .unwrap();
        let cropped =
            crate::audio_edit::trim_view(&original, crate::TrimEdge::Left, cut, &author).unwrap();
        let old_domain = &original.audio_edit().unwrap().domain;
        let domain = &cropped.audio_edit().unwrap().domain;
        // The guarded trim boundary now retains this context; no canonical
        // Outer admission, command history or native behavior is activated.
        assert_eq!(domain.fade, old_domain.fade);
        assert_eq!(
            domain.fade_projection.as_ref().unwrap().origin_beat_raw,
            old_domain.fade_projection.as_ref().unwrap().origin_beat_raw
        );
        assert_eq!(
            domain.outer_fade_displacement().unwrap(),
            i128::from(consumed)
        );
        let expected = original.projected_outer_fade(&author).unwrap().unwrap();
        let retained = cropped.projected_outer_fade(&author).unwrap().unwrap();
        for frame in (cut..original.start() + original.duration()).step_by(17) {
            assert_eq!(
                retained.gain_at(frame).unwrap(),
                expected.gain_at(frame).unwrap(),
                "frame {frame}"
            );
            assert_eq!(
                retained.domain_position_at_frame(frame).unwrap(),
                expected.domain_position_at_frame(frame).unwrap()
            );
        }
        let first_cut = author.beat_to_frame(BeatTime::ONE).unwrap();
        let first =
            crate::audio_edit::trim_view(&original, crate::TrimEdge::Left, first_cut, &author)
                .unwrap();
        let successive =
            crate::audio_edit::trim_view(&first, crate::TrimEdge::Left, cut, &author).unwrap();
        assert_eq!(
            successive.audio_edit().unwrap().domain,
            cropped.audio_edit().unwrap().domain
        );
        let mut wrong_context = cropped.clone();
        Arc::make_mut(&mut wrong_context.audio_edit_mut().unwrap().domain)
            .shift_outer_fade_displacement(-i128::from(consumed))
            .unwrap();
        assert_ne!(
            wrong_context
                .projected_outer_fade(&author)
                .unwrap()
                .unwrap()
                .gain_at(75_000)
                .unwrap(),
            expected.gain_at(75_000).unwrap(),
            "fixture must detect dropping full consumed ticks"
        );
    }
}

#[test]
fn displacement_helper_normalizes_rewinds_and_refuses_overflow_atomically() {
    let (mut clip, tempo) = repeated();
    clip.audio_edit_mut()
        .unwrap()
        .anchor_at_reference(0, 0, &tempo)
        .unwrap();
    let domain = Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain);
    let original = domain.clone();
    let one = i128::from(BeatTime::ONE.raw());
    domain.shift_outer_fade_displacement(one).unwrap();
    assert_eq!(
        (
            domain.fade_projection.as_ref().unwrap().local_start_raw,
            domain.fade_start
        ),
        (BeatTime::ONE.raw(), 0)
    );
    domain.shift_outer_fade_displacement(-2 * one).unwrap();
    assert_eq!(
        (
            domain.fade_projection.as_ref().unwrap().local_start_raw,
            domain.fade_start
        ),
        (0, BeatTime::ONE.raw())
    );
    domain.shift_outer_fade_displacement(one).unwrap();
    assert_eq!(*domain, original);
    let before = domain.clone();
    assert!(domain.shift_outer_fade_displacement(i128::MAX).is_err());
    assert_eq!(*domain, before);
    domain.fade_start = 1;
    domain.fade_projection.as_mut().unwrap().local_start_raw = 1;
    assert!(domain.outer_fade_displacement().is_err());
    assert!(clip.projected_outer_fade(&tempo).is_err());
}

#[test]
fn subframe_origins_survive_crop_and_common_rewind_with_observable_hard_gate() {
    let author = TempoMap::constant(48_000, Tempo::new(120_000_000).unwrap()).unwrap();
    let destination = TempoMap::constant(48_000, Tempo::new(12_000_000).unwrap()).unwrap();
    let origins = [0_i128, i128::from(BeatTime::ONE.raw() / 96_000)];
    let common_crop = i128::from(BeatTime::ONE.raw() / 4);
    let common_crop_frame =
        u64::try_from(audio_projection_frame(&author, common_crop).unwrap()).unwrap();
    assert_eq!(common_crop_frame, 6_000);
    let elapsed = destination.frame_to_beat(1).unwrap().raw();
    assert_eq!(elapsed, 17_896);
    assert!(0 < i128::from(elapsed) && i128::from(elapsed) < origins[1]);
    // Translate the immutable authoring origins by one beat to exercise
    // an equivalent collision for signed pre-zero authoring clocks.
    for origin_shift in [0, i128::from(BeatTime::ONE.raw())] {
        let mut old_representations = Vec::new();
        let mut gains = Vec::new();
        for placement_origin in origins {
            let author_origin = placement_origin - origin_shift;
            assert_eq!(
                audio_projection_frame(&author, author_origin).unwrap(),
                audio_projection_frame(&author, -origin_shift).unwrap()
            );
            let consumed = common_crop - placement_origin;
            old_representations.push((
                audio_projection_frame(&author, common_crop - origin_shift).unwrap()
                    - audio_projection_frame(&author, author_origin).unwrap(),
                author_origin + consumed,
            ));
            let (mut clip, _) = repeated();
            clip.audio_edit_mut()
                .unwrap()
                .anchor_at_reference(0, 0, &author)
                .unwrap();
            let edit = clip.audio_edit_mut().unwrap();
            edit.timing = AudioClipTiming::Musical(BeatTime::from_raw(
                u64::try_from(placement_origin).unwrap(),
            ));
            let domain = Arc::make_mut(&mut edit.domain);
            domain.fade = ClipFade::new(76_696, 0, 0, 3_600).unwrap();
            domain.fade_projection.as_mut().unwrap().origin_beat_raw = author_origin;
            crate::audio_edit::project_clip(&mut clip, &author).unwrap();
            let cropped = crate::audio_edit::trim_view(
                &clip,
                crate::TrimEdge::Left,
                common_crop_frame,
                &author,
            )
            .unwrap();

            let mut rewound =
                crate::audio_edit::trim_view(&cropped, crate::TrimEdge::Left, 0, &author).unwrap();
            let domain = &rewound.audio_edit().unwrap().domain;
            // Actual guarded trim previews retain context. Admission, command
            // history and native behavior remain unactivated.
            assert_eq!(domain.outer_fade_displacement().unwrap(), -placement_origin);
            assert_eq!(domain.fade.offset(), 0);
            assert_eq!(
                domain.fade_projection.as_ref().unwrap().origin_beat_raw,
                author_origin
            );
            crate::audio_edit::project_clip(&mut rewound, &destination).unwrap();
            gains.push(
                rewound
                    .projected_outer_fade(&destination)
                    .unwrap()
                    .unwrap()
                    .gain_at(1)
                    .unwrap(),
            );
        }
        assert_eq!(
            old_representations[0], old_representations[1],
            "mutable origin plus integer frame offset loses the distinction"
        );
        assert_eq!(
            gains,
            [1.0, 0.0],
            "subframe prefix is an audible gate, not technical metadata"
        );
    }
}

#[test]
fn hidden_retained_knots_near_upper_frame_limit_do_not_overflow_visible_projection() {
    let (mut clip, tempo) = repeated();
    clip.start = u64::MAX - clip.duration() - 10;
    Arc::make_mut(&mut clip.audio_edit_mut().unwrap().domain).fade =
        ClipFade::new(160_000, 0, 2_400, 3_600).unwrap();
    crate::audio_edit::project_clip(&mut clip, &tempo).unwrap();
    let view = AudioViewProjection::new(&clip, &tempo).unwrap();
    assert_eq!(view.outer_frame_bounds(), (clip.start(), u64::MAX - 10));
    assert_eq!(view.occurrence_count(), 4);
    let projection = clip.projected_outer_fade(&tempo).unwrap().unwrap();
    assert_eq!(projection.frame_at_domain_position(159_999).unwrap(), None);
    assert_eq!(
        projection.frame_at_domain_position(2_399).unwrap(),
        Some(clip.start() + 2_399)
    );
    assert!(
        projection
            .breakpoints()
            .unwrap()
            .into_iter()
            .flatten()
            .all(|frame| (clip.start()..=u64::MAX - 10).contains(&frame))
    );
}

mod crop_tests;

mod route_tests;
