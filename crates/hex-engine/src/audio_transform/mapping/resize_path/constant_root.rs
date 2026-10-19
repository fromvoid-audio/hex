//! First actual mapping certificate: full processing root at one constant tempo.
//! A deliberately partial RESEARCH certificate, not a supported-shape refusal.
//! Other topologies return uncertain; no UI or shipping enumeration fallback.
use super::rounded::Rounded;
use hex_project::{AudioDomainClock, BeatsPreserve, Clip, TempoMap, TrimEdge, WarpMode};

pub(super) fn certify(
    original: &Clip,
    edge: TrimEdge,
    low: u64,
    high: u64,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Option<bool> {
    if low > high || tempo.points().len() != 1 {
        return Some(false);
    }
    let edit = original.audio_edit()?;
    let domain = &edit.domain;
    let AudioDomainClock::Beats(span) = domain.clock else {
        return Some(false);
    };
    if !domain.warp_enabled
        || domain.events.is_some()
        || edit.view_start != 0
        || edit.view_end != span.raw()
        || domain.source_phase != 0
        || domain.source_length > (1_u64 << 40)
        || domain.fade_start != 0
        || !domain.fade.is_neutral()
        || domain.fade.offset() != 0
        || domain.fade.span() != original.duration()
    {
        return Some(false);
    }
    // Constructor/model checks are independent of ratio proof. On this restricted
    // full-root family, retiming leaves source fields/gain/mode unchanged, gives
    // clock=view_end=N exactly, and fade span becomes the exact new root frames.
    edit.validate(original, tempo, asset_frames).ok()?;
    let warp = domain.warp.as_ref()?;
    if warp.markers.last()?.beat.raw() != span.raw() {
        return Some(false);
    }
    let original_end = original.start().checked_add(original.duration())?;
    let (short_start, short_end, long_start, long_end) = match edge {
        TrimEdge::Right if low > original.start() => {
            (original.start(), low, original.start(), high)
        }
        TrimEdge::Left if high < original_end => (high, original_end, low, original_end),
        _ => return Some(false),
    };
    let shortest = short_end.checked_sub(short_start)?;
    let longest = long_end.checked_sub(long_start)?;
    if shortest == 0 || longest > hex_project::MAX_AUDIO_DOMAIN_FRAMES {
        return Some(false);
    }
    let beat_span = |start, end| {
        tempo
            .frame_to_beat(end)
            .ok()?
            .raw()
            .checked_sub(tempo.frame_to_beat(start).ok()?.raw())
    };
    let n_low = beat_span(short_start, short_end)?;
    let n_high = beat_span(long_start, long_end)?;
    if n_low == 0 || n_low > n_high {
        return Some(false);
    }
    if let WarpMode::Beats {
        preserve: BeatsPreserve::Division(division),
        ..
    } = warp.mode
        && (division.raw() == 0
            || n_high / division.raw() + 1 > hex_project::MAX_AUDIO_POINTS as u64)
    {
        return Some(false);
    }
    // With zero phase and unit==root, loop repetitions contribute no internal
    // boundary. Raw boundary count is exactly markers.len(), below8192 by model
    // admission. All source endpoints remain within the original admitted range.
    let mode = super::super::super::render_mode(domain).ok()?;
    for pair in warp.markers.windows(2) {
        let mut difference = Rounded::new(span.raw(), 0, 1, false)?;
        difference.add_scaled(pair[1].beat.raw(), 1)?;
        difference.add_scaled(pair[0].beat.raw(), -1)?;
        let (beat_low, beat_high) = difference.bounds(n_low, n_high)?;
        if beat_low <= 0 {
            return Some(false);
        }
        let (frames_low, frames_high) = tempo
            .same_tempo_frame_span_bounds(
                tempo.points()[0].tempo(),
                u64::try_from(beat_low).ok()?,
                u64::try_from(beat_high).ok()?,
            )
            .ok()?;
        if frames_low == 0 {
            return Some(false);
        }
        let source_span = i128::from(pair[1].source_frame.checked_sub(pair[0].source_frame)?) << 32;
        // Same exact float order, pitch compensation and epsilon as rendering.
        // Both extrema accepting proves every positive intermediate tuple accepts.
        if hex_warp::validate_stretch_segment(frames_low, source_span, mode, domain.pitch_cents)
            .is_err()
            || hex_warp::validate_stretch_segment(
                frames_high,
                source_span,
                mode,
                domain.pitch_cents,
            )
            .is_err()
        {
            return Some(false);
        }
    }
    Some(true)
}

#[test]
fn every_small_certified_interval_is_canonically_valid() {
    use hex_project::{
        Command, Created, Editor, Project, default_audio_warp, preview_audio_clip_stretch,
    };
    let mut certified = 0;
    for pitch in [-1_200, 0, 1_200] {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "certificate".into(),
                path: "certificate.wav".into(),
                sample_rate: 16_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let mut settings = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: hex_project::BeatsTail::Forward,
                decay: 100,
            },
        )
        .unwrap();
        settings.markers.insert(
            1,
            hex_project::WarpMarker {
                source_frame: 16,
                beat: hex_project::BeatTime::from_raw(settings.markers[1].beat.raw() / 2),
            },
        );
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: pitch })
            .unwrap();
        let original = editor.project().clip(clip).unwrap();
        let tempo = editor.project().tempo_map();
        let truth: Vec<bool> = (0..=96)
            .map(|frame| {
                preview_audio_clip_stretch(original, TrimEdge::Right, frame, tempo, 32).is_ok_and(
                    |candidate| {
                        super::super::super::validate_clip_warp_preview(&candidate, tempo).is_ok()
                    },
                )
            })
            .collect();
        for low in 0..=96 {
            for high in low..=96 {
                if certify(
                    original,
                    TrimEdge::Right,
                    low as u64,
                    high as u64,
                    tempo,
                    32,
                ) == Some(true)
                {
                    assert!(
                        truth[low..=high].iter().all(|valid| *valid),
                        "false proof pitch={pitch} interval={low}..{high}"
                    );
                    certified += 1;
                }
            }
        }
        let mut frontier = super::Frontier::new(32);
        let mut work = super::Work::default();
        for request in [96, 0, 32, 65, 9, 96] {
            let expected_left = (0..32).rev().find(|&f| !truth[f]).map_or(0, |f| f + 1);
            let expected_right = (33..=96).find(|&f| !truth[f]).map_or(96, |f| f - 1);
            let actual = frontier
                .request(
                    request,
                    &mut |lo, hi| {
                        Ok::<_, ()>(
                            certify(original, TrimEdge::Right, lo, hi, tempo, 32) == Some(true),
                        )
                    },
                    &mut |frame| Ok(truth[frame as usize]),
                    &mut work,
                )
                .unwrap();
            assert_eq!(
                actual,
                request.clamp(expected_left as u64, expected_right as u64)
            );
        }
        eprintln!("actual constant-root certificate pitch={pitch} work={work:?}");
    }
    assert!(certified > 1_000, "must prove real non-singleton intervals");
    eprintln!("actual mapping certified intervals={certified}, all exhaustively checked");
}

#[test]
fn maximum_marker_shape_profiles_requested_prefix_without_frame_scan() {
    use hex_project::{
        Command, Created, Editor, Project, default_audio_warp, preview_audio_clip_stretch,
    };
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let frames = 2_000_000_u64;
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "max markers".into(),
            path: "max-markers.wav".into(),
            sample_rate: 16_000,
            frames,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: hex_project::BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    let unit = settings.markers[1].beat.raw();
    settings.markers = (0..hex_project::MAX_AUDIO_POINTS)
        .map(|index| {
            let index = index as u128;
            let denominator = (hex_project::MAX_AUDIO_POINTS - 1) as u128;
            hex_project::WarpMarker {
                source_frame: (u128::from(frames) * index / denominator) as u64,
                beat: hex_project::BeatTime::from_raw(
                    (u128::from(unit) * index / denominator) as u64,
                ),
            }
        })
        .collect();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap();
    let tempo = editor.project().tempo_map();
    let mut frontier = super::Frontier::new(frames);
    let mut work = super::Work::default();
    let admitted = |frame| {
        preview_audio_clip_stretch(original, TrimEdge::Right, frame, tempo, frames).is_ok_and(
            |candidate| super::super::super::validate_clip_warp_preview(&candidate, tempo).is_ok(),
        )
    };
    let started = std::time::Instant::now();
    let high = frontier
        .request(
            12_000_000,
            &mut |lo, hi| {
                Ok::<_, ()>(certify(original, TrimEdge::Right, lo, hi, tempo, frames) == Some(true))
            },
            &mut |f| Ok(admitted(f)),
            &mut work,
        )
        .unwrap();
    assert_eq!(high, 12_000_000);
    let low = frontier
        .request(
            0,
            &mut |lo, hi| {
                Ok::<_, ()>(certify(original, TrimEdge::Right, lo, hi, tempo, frames) == Some(true))
            },
            &mut |f| Ok(admitted(f)),
            &mut work,
        )
        .unwrap();
    assert!(admitted(low));
    assert!(!admitted(low - 1));
    let before = work;
    assert_eq!(
        frontier
            .request(
                frames,
                &mut |_, _| Ok::<_, ()>(false),
                &mut |_| Ok(false),
                &mut work
            )
            .unwrap(),
        frames
    );
    assert_eq!(before, work);
    eprintln!(
        "max-marker profile markers={} requested=[0,12000000] connected=[{low},{high}] work={work:?} elapsed={:?}",
        hex_project::MAX_AUDIO_POINTS,
        started.elapsed()
    );
    // Research measurement oracle, NOT a runtime refusal or shipping budget.
    assert!(
        work.singletons < 100,
        "certificate degeneration: report this counterexample before UI work"
    );
}
