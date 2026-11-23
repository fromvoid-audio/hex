//! Geometry and edits for clip fade handles, independent of pointer capture.
use hex_project::ClipFade;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    InTop,
    InBottom,
    InCurve,
    OutTop,
    OutBottom,
    OutCurve,
}
impl Handle {
    pub fn curve_side(self) -> Option<bool> {
        match self {
            Self::InCurve => Some(false),
            Self::OutCurve => Some(true),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::InTop => "fade-in-top",
            Self::InBottom => "fade-in-bottom",
            Self::InCurve => "fade-in-curve",
            Self::OutTop => "fade-out-top",
            Self::OutBottom => "fade-out-bottom",
            Self::OutCurve => "fade-out-curve",
        }
    }
}
// Curve motion retains subframe precision; endpoint lengths still round to frames.
pub fn edited_fade(fade: ClipFade, handle: Handle, frames: f64, bend: i16) -> Option<ClipFade> {
    if !frames.is_finite() {
        return None;
    }
    let whole_frames = frames.round() as i128;
    let span = fade.span();
    let mut fade_in = fade.fade_in().min(span);
    let mut fade_out = fade.fade_out().min(span);
    let mut in_curve = fade.in_curve();
    let mut out_curve = fade.out_curve();
    let mut in_position = fade.in_position();
    let mut out_position = fade.out_position();
    // Bottom anchors stay fixed at clip corners (`in_start = 0`,
    // `out_end_offset = 0`, `offset = 0`). The top handles push each other:
    // the dragged ramp reaches the opposite corner and the other ramp gives
    // way. Recomputing from the drag's original fade springs the pushed ramp
    // back to, and no further than, its pre-drag length when the same gesture
    // reverses.
    let body = span;
    match handle {
        Handle::InTop => {
            fade_in = i128::from(fade_in)
                .saturating_add(whole_frames)
                .clamp(0, i128::from(body)) as u64;
            fade_out = fade_out.min(body - fade_in);
        }
        Handle::OutTop => {
            fade_out = i128::from(fade_out)
                .saturating_sub(whole_frames)
                .clamp(0, i128::from(body)) as u64;
            fade_in = fade_in.min(body - fade_out);
        }
        Handle::InBottom | Handle::OutBottom => return None,
        Handle::InCurve | Handle::OutCurve => {
            let out = handle == Handle::OutCurve;
            let curve = if out { &mut out_curve } else { &mut in_curve };
            // The diamond stays between channel centers (central half of body).
            // Opening a legacy bend never changes it. An explicit horizontal
            // edit fits hidden extremes to the visible ordinate, so all four
            // corners remain reachable without moving the diamond vertically.
            if bend != 0 {
                let control = (*curve).clamp(-500, 500);
                let next = control.saturating_add(bend).clamp(-500, 500);
                // Legacy bends can lie beyond the visual band. Start movement
                // from the visible edge, with no dead zone or outward-only edit.
                if next != control || *curve == control {
                    *curve = next;
                }
            }
            if frames != 0.0 {
                *curve = (*curve).clamp(-500, 500);
            }
            let length = if out { fade_out } else { fade_in };
            let changed = if out {
                out_curve != fade.out_curve()
            } else {
                in_curve != fade.in_curve()
            };
            if length > 1 && (frames != 0.0 || changed) {
                let original = fade.position_at_gain(out, curve_handle_gain(fade, out))?;
                let target = (original + frames / (length - 1) as f64).clamp(0.001, 0.999);
                // Explicit edits use the bounded ordinate, whose mapped half
                // progress is the canonical horizontal position.
                let position = (target * 1000.0).round().clamp(1.0, 999.0) as u16;
                if out {
                    out_position = position;
                } else {
                    in_position = position;
                }
            }
        }
    }
    ClipFade::new(span, 0, fade_in, fade_out)
        .ok()?
        .with_shape(0, 0, in_curve, out_curve)
        .ok()?
        .with_curve_positions(in_position, out_position)
        .ok()
        .map(|next| {
            next.with_curve_laws(fade.curve_laws())
                .with_new_ramp_defaults(fade)
        })
}

// Each handle belongs to its own ramp, including legacy bends. Overlapping
// ramps multiply in playback/the shaded envelope, but must not pull the other
// ramp's control outside its editing box.
pub fn curve_handle_gain(fade: ClipFade, out: bool) -> f64 {
    fade.curve_midpoint_gain(out).clamp(0.25, 0.75)
}

pub fn knots(fade: ClipFade) -> [(Handle, (u64, f64), f32); 6] {
    let span = fade.span().max(1);
    let end = span - 1;
    let in_len = fade.fade_in().min(span);
    let out_len = fade.fade_out().min(span);
    let in_top = in_len.saturating_sub(1);
    let out_top = span.saturating_sub(out_len).min(end);
    let interior = |start: u64, end: u64, out| {
        let length = end.saturating_sub(start);
        let offset = length as f64
            * fade
                .position_at_gain(out, curve_handle_gain(fade, out))
                .unwrap_or(0.5);
        let whole = (offset.floor() as u64).min(length);
        (start + whole, (offset - whole as f64).clamp(0.0, 1.0))
    };
    [
        (Handle::InTop, (in_top, 0.0), 1.0),
        (Handle::InBottom, (0, 0.0), 0.0),
        (
            Handle::InCurve,
            interior(0, in_top, false),
            curve_handle_gain(fade, false) as f32,
        ),
        (Handle::OutTop, (out_top, 0.0), 1.0),
        (Handle::OutBottom, (end, 0.0), 0.0),
        (
            Handle::OutCurve,
            interior(out_top, end, true),
            curve_handle_gain(fade, true) as f32,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_lengths_and_curves_remain_independent() {
        let fade = ClipFade::new(100, 0, 40, 30)
            .unwrap()
            .with_shape(10, 10, 0, 0)
            .unwrap();
        let bent = edited_fade(fade, Handle::InCurve, 0.0, 700).unwrap();
        assert_eq!(
            (
                bent.in_start(),
                bent.out_end_offset(),
                bent.fade_in(),
                bent.fade_out()
            ),
            (0, 0, 40, 30)
        );
        assert!(bent.gain_at(20) < fade.reanchor_to_corners(100).unwrap().gain_at(20));
        let pushed = edited_fade(bent, Handle::InTop, (i128::MAX - 100) as f64, 0).unwrap();
        assert_eq!((pushed.fade_in(), pushed.fade_out()), (100, 0));
        let pushed = edited_fade(bent, Handle::OutTop, i128::MIN as f64, 0).unwrap();
        assert_eq!((pushed.fade_in(), pushed.fade_out()), (0, 100));
    }

    #[test]
    fn top_handles_push_each_other_and_spring_back() {
        let original = ClipFade::new(100, 0, 30, 40)
            .unwrap()
            .with_shape(5, 5, 300, -200)
            .unwrap()
            .with_curve_positions(400, 600)
            .unwrap();
        let lengths = |handle, frames| {
            let fade = edited_fade(original, handle, frames, 0).unwrap();
            assert_eq!(
                (
                    fade.in_start(),
                    fade.out_end_offset(),
                    fade.in_curve(),
                    fade.out_curve(),
                    fade.in_position(),
                    fade.out_position()
                ),
                (0, 0, 300, -200, 400, 600)
            );
            (fade.fade_in(), fade.fade_out())
        };
        assert_eq!(lengths(Handle::InTop, 20.0), (50, 40));
        assert_eq!(lengths(Handle::InTop, 35.0), (65, 35));
        assert_eq!(lengths(Handle::InTop, 10.0), (40, 40));
        assert_eq!(lengths(Handle::InTop, -30.0), (0, 40));
        assert_eq!(lengths(Handle::InTop, 1e9), (100, 0));
        assert_eq!(lengths(Handle::OutTop, -35.0), (25, 75));
        assert_eq!(lengths(Handle::OutTop, -10.0), (30, 50));
        assert_eq!(lengths(Handle::OutTop, 50.0), (30, 0));
        assert_eq!(lengths(Handle::OutTop, -1e9), (0, 100));
        let legacy = ClipFade::new(100, 0, 70, 60).unwrap();
        let fade = edited_fade(legacy, Handle::InTop, 0.0, 0).unwrap();
        assert_eq!((fade.fade_in(), fade.fade_out()), (70, 30));
        let empty = ClipFade::new(10, 0, 0, 0)
            .unwrap()
            .with_shape(5, 5, 0, 0)
            .unwrap();
        for handle in [Handle::InTop, Handle::OutTop] {
            let fade = edited_fade(empty, handle, 7.0, 0).unwrap();
            assert_eq!((fade.in_start(), fade.out_end_offset()), (0, 0));
            assert!(fade.fade_in() + fade.fade_out() <= 10);
        }
    }

    #[test]
    fn knots_and_edited_fade_always_anchor_bottom_handles_to_corners_and_stay_in_bounds() {
        let legacy = ClipFade::new(100, 0, 30, 25)
            .unwrap()
            .with_shape(15, 20, 250, -350)
            .unwrap();
        let projected = knots(legacy);
        for (handle, (frame, frac), _) in projected {
            assert!(
                frame < legacy.span(),
                "handle {handle:?} frame {frame} out of clip bounds {}",
                legacy.span()
            );
            assert!((0.0..=1.0).contains(&frac));
            match handle {
                Handle::InBottom => assert_eq!((frame, frac), (0, 0.0)),
                Handle::OutBottom => assert_eq!((frame, frac), (legacy.span() - 1, 0.0)),
                _ => {}
            }
        }
        for handle in [
            Handle::InTop,
            Handle::OutTop,
            Handle::InCurve,
            Handle::OutCurve,
        ] {
            let edited = edited_fade(legacy, handle, 5.0, -50).unwrap();
            assert_eq!(
                (
                    edited.offset(),
                    edited.span(),
                    edited.in_start(),
                    edited.out_end_offset()
                ),
                (0, 100, 0, 0)
            );
            assert!(edited.fade_in() + edited.fade_out() <= edited.span());
        }
    }

    #[test]
    fn overlapping_fades_keep_each_control_inside_its_own_box() {
        let fade = ClipFade::new(101, 0, 101, 101)
            .unwrap()
            .with_shape(0, 0, 500, -500)
            .unwrap()
            .with_curve_positions(250, 750)
            .unwrap();
        for (handle, _, gain) in knots(fade) {
            if matches!(handle, Handle::InCurve | Handle::OutCurve) {
                assert!((0.25..=0.75).contains(&gain));
            }
        }
    }

    #[test]
    fn rectangular_corners_are_independent_and_legacy_handles_stay_on_curve() {
        let original = ClipFade::new(10001, 0, 5001, 5000).unwrap();
        for original in [original, original.with_shape(0, 0, 1000, -1000).unwrap()] {
            for handle in [Handle::InCurve, Handle::OutCurve] {
                for dx in [-100000, 100000] {
                    for dy in [-2000, 2000] {
                        let edited = edited_fade(original, handle, f64::from(dx), dy).unwrap();
                        let out = handle == Handle::OutCurve;
                        let gain = curve_handle_gain(edited, out);
                        let x = edited.position_at_gain(out, gain).unwrap();
                        assert!((gain - if dy > 0 { 0.25 } else { 0.75 }).abs() < 0.001);
                        assert!((x - if dx > 0 { 0.999 } else { 0.001 }).abs() < 0.001);
                        assert_eq!((edited.fade_in(), edited.fade_out()), (5001, 5000));
                    }
                }
            }
        }
        let legacy = original.with_shape(0, 0, 900, -900).unwrap();
        assert_eq!(edited_fade(legacy, Handle::InCurve, 0.0, 10), Some(legacy));
        let inward = edited_fade(legacy, Handle::InCurve, 0.0, -10).unwrap();
        assert!(curve_handle_gain(inward, false) > curve_handle_gain(legacy, false));
        let moved = edited_fade(legacy, Handle::InCurve, 500.0, 0).unwrap();
        assert!((curve_handle_gain(moved, false) - curve_handle_gain(legacy, false)).abs() < 0.001);
        let before = legacy
            .position_at_gain(false, curve_handle_gain(legacy, false))
            .unwrap();
        let after = moved
            .position_at_gain(false, curve_handle_gain(moved, false))
            .unwrap();
        assert!((after - before - 0.1).abs() < 0.002);
    }
}
