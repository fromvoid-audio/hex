use hex_project::{Clip, ClipFade, ClipId, Command, Created, Editor, Project};
use std::path::PathBuf;

// Audible envelope of one part: an inner event fade (root clock) times the
// part's clip fade. A cut ramp lives on the event so no clip fade overhangs.
fn envelope_gain(part: &Clip, frame: u64) -> f32 {
    let Some(edit) = part.audio_edit() else {
        return part.fade().gain_at(frame);
    };
    let root = edit.view_start + frame;
    let events = part.audio_events();
    let event = events
        .events
        .iter()
        .find(|event| event.start <= root && root < event.end)
        .expect("event under the view");
    let inner = event.fade.map_or(1.0, |fade| {
        fade.shape()
            .gain_at(event.source_start + (root - event.start) - fade.source_start())
    });
    let outer = root
        .checked_sub(edit.domain.fade_start)
        .map_or(0.0, |age| edit.domain.fade.gain_at(age));
    inner * outer
}

fn editor_with_clip(duration: u64) -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("audio.wav"),
            sample_rate: 48_000,
            frames: duration,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration,
            source_offset: 0,
            source_length: duration,
            looping: false,
        })
        .expect("clip")
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn endpoint_laws_overlap_single_frames_and_large_domains() {
    let zero = ClipFade::neutral(4).unwrap();
    assert_eq!(
        (0..4).map(|frame| zero.gain_at(frame)).collect::<Vec<_>>(),
        [1.0; 4]
    );

    let fade_in = ClipFade::new(5, 0, 3, 0).unwrap();
    assert_eq!(fade_in.gain_at(0), 0.0);
    assert_eq!(fade_in.gain_at(1), 0.5);
    assert_eq!(fade_in.gain_at(2), 1.0);
    assert_eq!(fade_in.gain_at(4), 1.0);

    let fade_out = ClipFade::new(5, 0, 0, 3).unwrap();
    assert_eq!(fade_out.gain_at(2), 1.0);
    assert_eq!(fade_out.gain_at(3), 0.5);
    assert_eq!(fade_out.gain_at(4), 0.0);

    let overlap = ClipFade::new(4, 0, 4, 4).unwrap();
    assert_eq!(overlap.gain_at(0), 0.0);
    assert!((overlap.gain_at(1) - 2.0 / 9.0).abs() < 1e-7);
    assert_eq!(overlap.gain_at(3), 0.0);

    assert_eq!(ClipFade::new(1, 0, 1, 0).unwrap().gain_at(0), 1.0);
    assert_eq!(ClipFade::new(1, 0, 0, 1).unwrap().gain_at(0), 0.0);

    let huge = ClipFade::new(u64::MAX, 0, 0, u64::MAX).unwrap();
    assert!(huge.gain_at(u64::MAX - 2) > 0.0);
    assert_eq!(huge.gain_at(u64::MAX - 1), 0.0);
}

#[test]
fn malformed_domains_and_fade_commands_are_atomic_and_undoable() {
    assert!(ClipFade::new(0, 0, 0, 0).is_err());
    assert!(ClipFade::new(4, 4, 0, 0).is_err());
    assert!(ClipFade::new(4, u64::MAX, 0, 0).is_err());
    assert!(ClipFade::new(4, 0, 5, 0).is_err());
    assert!(ClipFade::new(4, 0, 0, 5).is_err());
    assert!(
        ClipFade::new(8, 5, 0, 0)
            .unwrap()
            .validate_segment(4)
            .is_err()
    );
    assert!(
        ClipFade::new(8, 1, 0, 0)
            .unwrap()
            .validate_segment(u64::MAX)
            .is_err()
    );

    let (valid, existing_clip) = editor_with_clip(8);
    let mut snapshot = valid.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].fade = ClipFade::new(8, 7, 0, 0).unwrap();
    assert!(Project::from_snapshot(snapshot).is_err());
    let mut snapshot = valid.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].duration = u64::MAX;
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].fade =
        ClipFade::new(u64::MAX, 1, 0, 0).unwrap();
    assert!(Project::from_snapshot(snapshot).is_err());
    assert!(valid.project().clip(existing_clip).is_some());

    let (mut editor, clip) = editor_with_clip(8);
    let original = editor.project().clip(clip).unwrap().fade();
    let before = editor.project().clone();
    let can_undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 9,
                fade_out: 0,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert_eq!(editor.can_undo(), can_undo);

    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 7,
            fade_out: 6,
        })
        .expect("set fades");
    assert_eq!(
        editor.project().clip(clip).unwrap().fade(),
        ClipFade::new(8, 0, 7, 6).unwrap()
    );
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert!(editor.redo());
    assert_eq!(editor.project().clip(clip).unwrap().fade().fade_in(), 7);
}

#[test]
fn duplicate_and_move_preserve_domain_while_trim_reanchors_and_clamps() {
    let (mut editor, clip) = editor_with_clip(8);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 6,
            fade_out: 5,
        })
        .unwrap();
    let original_fade = editor.project().clip(clip).unwrap().fade();
    let duplicate = match editor.execute(Command::DuplicateClip(clip)).unwrap() {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(
        editor.project().clip(duplicate).unwrap().fade(),
        original_fade
    );
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: duplicate,
            track,
            start: 20,
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(duplicate).unwrap().fade(),
        original_fade
    );

    editor
        .execute(Command::TrimClip {
            clip,
            start: 0,
            duration: 4,
            source_offset: 0,
            source_length: 4,
        })
        .unwrap();
    assert_eq!(
        editor.project().clip(clip).unwrap().fade(),
        ClipFade::new(4, 0, 4, 4).unwrap()
    );
}

#[test]
fn recursive_split_preserves_every_sample_of_original_envelope() {
    let (mut editor, clip) = editor_with_clip(11);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 7,
            fade_out: 6,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap().fade();
    editor
        .execute(Command::SplitClip { clip, frame: 4 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 8,
        })
        .unwrap();
    let segments = editor.project().tracks()[0].clips();
    assert_eq!(segments.len(), 3);
    // The first cut crossed the fade in: its part keeps no overhanging clip fade.
    assert_eq!(
        (
            segments[0].retained_fade().span(),
            segments[0].retained_fade().offset()
        ),
        (4, 0)
    );
    let mut absolute = 0;
    for segment in segments {
        for local in 0..segment.duration() {
            assert_eq!(
                envelope_gain(segment, local),
                original.gain_at(absolute + local)
            );
        }
        absolute += segment.duration();
    }
    assert_eq!(absolute, 11);
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap().fade(), original);
    assert!(editor.redo());
    assert!(editor.redo());
}

#[test]
fn shaped_fades_have_independent_silent_endpoints_curves_and_split_continuity() {
    let fade = ClipFade::new(101, 0, 41, 31)
        .unwrap()
        .with_shape(10, 10, 500, -500)
        .unwrap();
    assert_eq!(fade.gain_at(9), 0.0);
    assert_eq!(fade.gain_at(10), 0.0);
    assert_eq!(fade.gain_at(50), 1.0);
    assert!((fade.gain_at(30) - 501.0 / 2002.0).abs() < 1e-7);
    assert!((fade.gain_at(75) - 1501.0 / 2002.0).abs() < 1e-7);
    assert_eq!(fade.gain_at(90), 0.0);
    assert_eq!(fade.gain_at(91), 0.0);
    for curve in [-1000, -500, 0, 500, 1000] {
        let ramp = ClipFade::new(101, 0, 101, 0)
            .unwrap()
            .with_shape(0, 0, curve, 0)
            .unwrap();
        let gains: Vec<_> = (0..101).map(|frame| ramp.gain_at(frame)).collect();
        assert!(gains.windows(2).all(|g| g[0] <= g[1]));
        assert_eq!((gains[0], gains[100]), (0.0, 1.0));
    }
    assert!(fade.with_shape(70, 0, 0, 0).is_err());
    assert!(fade.with_shape(0, u64::MAX, 0, 0).is_err());
    assert!(fade.with_shape(0, 0, 1001, 0).is_err());
    let (mut editor, clip) = editor_with_clip(101);
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    assert!(editor.undo());
    assert!(editor.project().clip(clip).unwrap().fade().is_neutral());
    assert!(editor.redo());
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: fade.with_offset(1, 100).unwrap()
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    editor
        .execute(Command::SplitClip { clip, frame: 35 })
        .unwrap();
    for part in editor.project().tracks()[0].clips() {
        for frame in 0..part.duration() {
            assert_eq!(
                envelope_gain(part, frame),
                fade.gain_at(part.start() + frame)
            );
        }
    }
    let trimmed = fade.reanchor(20).unwrap();
    assert_eq!(
        (
            trimmed.in_start(),
            trimmed.out_end_offset(),
            trimmed.fade_in(),
            trimmed.fade_out()
        ),
        (10, 10, 10, 10)
    );
    assert_eq!((trimmed.in_curve(), trimmed.out_curve()), (500, -500));
}

#[test]
fn two_axis_fades_are_independent_monotone_and_preserve_centered_legacy_pcm() {
    for curve in [-1000, -500, 0, 500, 1000] {
        let original = ClipFade::new(1001, 0, 1001, 0)
            .unwrap()
            .with_shape(0, 0, curve, 0)
            .unwrap();
        for frame in 0..=1000 {
            let t = frame as f64 / 1000.0;
            let c = f64::from(curve);
            let legacy = if curve == 0 {
                t
            } else {
                t * (1001.0 - c) / (1001.0 + c - 2.0 * c * t)
            } as f32;
            assert_eq!(original.gain_at(frame), legacy);
        }
        for position in [1, 100, 137, 250, 321, 500, 750, 900, 999] {
            let fade_in = original.with_curve_positions(position, 500).unwrap();
            let fade_out = ClipFade::new(1001, 0, 0, 1001)
                .unwrap()
                .with_shape(0, 0, 0, curve)
                .unwrap()
                .with_curve_positions(500, 1000 - position)
                .unwrap();
            let mut prior = 0.0;
            for frame in 0..=1000 {
                let gain = fade_in.gain_at(frame);
                assert!(gain.is_finite() && (0.0..=1.0).contains(&gain));
                assert!(
                    gain >= prior,
                    "curve {curve}, position {position}, frame {frame}"
                );
                assert_eq!(gain, fade_out.gain_at(1000 - frame));
                prior = gain;
            }
            assert_eq!((fade_in.gain_at(0), fade_in.gain_at(1000)), (0.0, 1.0));
            let anchor = fade_in.gain_at(u64::from(position));
            assert_eq!(fade_in.curve_midpoint_gain(false) as f32, anchor);
            assert_eq!(fade_out.curve_midpoint_gain(true) as f32, anchor);
            assert!((anchor - (1001.0 - f32::from(curve)) / 2002.0).abs() < 1e-6);
            for gain in [0.25, 0.5, 0.75] {
                let x = fade_in.position_at_gain(false, gain).unwrap();
                let frame = (x * 1000.0).round() as u64;
                // The inverse agrees with the evaluated frame within sampling error.
                let below = fade_in.gain_at(frame.saturating_sub(1));
                let above = fade_in.gain_at((frame + 1).min(1000));
                assert!(below <= gain as f32 + 1e-6 && above >= gain as f32 - 1e-6);
            }
        }
    }
    let fade = ClipFade::new(1001, 0, 1001, 0).unwrap();
    let early = fade.with_curve_positions(250, 500).unwrap();
    let late = fade.with_curve_positions(750, 500).unwrap();
    assert_eq!(early.gain_at(250), 0.5);
    assert!(late.gain_at(250) < fade.gain_at(250));
    assert!(early.gain_at(500) > fade.gain_at(500));
    let bent = early.with_shape(0, 0, 500, 0).unwrap();
    assert!((bent.gain_at(250) - 501.0 / 2002.0).abs() < 1e-7);
    // A single rational bend cannot reproduce the independently movable interior
    // slope: at the same half-gain point the time-warped ramp has different tails.
    assert_ne!(early.gain_at(750), bent.gain_at(750));
    for invalid in [0, 1000, u16::MAX] {
        assert!(fade.with_curve_positions(invalid, 500).is_err());
        assert!(fade.with_curve_positions(500, invalid).is_err());
    }
    assert!(fade.position_at_gain(false, f64::NAN).is_none());
}

#[test]
fn two_axis_command_history_split_and_reanchor_retain_shape() {
    let (mut editor, clip) = editor_with_clip(101);
    let fade = ClipFade::new(101, 0, 41, 31)
        .unwrap()
        .with_shape(10, 10, 200, -300)
        .unwrap()
        .with_curve_positions(250, 700)
        .unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    editor
        .execute(Command::SplitClip { clip, frame: 35 })
        .unwrap();
    let parts = editor.project().tracks()[0].clips();
    // The crossed fade in moves to the inner event; the fade out stays local.
    let inner = parts[0].audio_events().events[0].fade.unwrap().shape();
    assert_eq!((inner.in_position(), inner.in_curve()), (250, 200));
    assert_eq!(
        (
            parts[1].retained_fade().out_position(),
            parts[1].retained_fade().out_curve()
        ),
        (700, -300)
    );
    for part in parts {
        for frame in 0..part.duration() {
            assert_eq!(
                envelope_gain(part, frame),
                fade.gain_at(part.start() + frame)
            );
        }
    }
    let fit = fade.reanchor(20).unwrap();
    assert_eq!((fit.in_position(), fit.out_position()), (250, 700));
}

#[test]
fn off_center_shapes_concentrate_transitions_with_long_quiet_or_loud_tails() {
    let early = ClipFade::new(1001, 0, 0, 1001)
        .unwrap()
        .with_shape(0, 0, 0, -500)
        .unwrap()
        .with_curve_positions(500, 150)
        .unwrap();
    assert!((early.gain_at(150) - 1501.0 / 2002.0).abs() < 1e-6);
    assert!(
        early.gain_at(500) < 0.1,
        "early S fade must settle into its quiet tail"
    );
    assert!(early.gain_at(750) < 0.005);
    let late = ClipFade::new(1001, 0, 0, 1001)
        .unwrap()
        .with_shape(0, 0, 0, 500)
        .unwrap()
        .with_curve_positions(500, 850)
        .unwrap();
    assert!(late.gain_at(250) > 0.995);
    assert!(
        late.gain_at(500) > 0.9,
        "late S fade retains its loud opening"
    );
    assert!((late.gain_at(850) - 501.0 / 2002.0).abs() < 1e-6);
}

#[test]
fn smoothstep_ramps_are_monotone_and_equal_gain() {
    use hex_project::FadeCurveLaw::Linear;
    let incoming = ClipFade::new(101, 0, 101, 0)
        .unwrap()
        .with_curve_laws([ClipFade::DEFAULT_S_CURVE, Linear]);
    let outgoing = ClipFade::new(101, 0, 0, 101)
        .unwrap()
        .with_curve_laws([Linear, ClipFade::DEFAULT_S_CURVE]);
    let mut previous = 0.0;
    for frame in 0..=100 {
        let gain = incoming.gain_at(frame);
        assert!(gain >= previous);
        assert_eq!(gain, outgoing.gain_at(100 - frame));
        assert!((gain + outgoing.gain_at(frame) - 1.0).abs() <= f32::EPSILON);
        assert!(
            (incoming.position_at_gain(false, f64::from(gain)).unwrap() - frame as f64 / 100.0)
                .abs()
                < 1e-6
        );
        previous = gain;
    }
    assert_eq!(
        [0, 25, 50, 75, 100].map(|frame| incoming.gain_at(frame)),
        [0.0, 0.15625, 0.5, 0.84375, 1.0]
    );
}
