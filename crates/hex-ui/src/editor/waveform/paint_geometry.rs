//! Bounded peak ordinates and fade sampling for waveform paint and previews.
use hex_project::{Clip, ClipFade};
use hex_session::CachedPeak;

const FADE_ENVELOPE_SAMPLES: usize = 32;
const FADE_ENVELOPE_BREAKPOINTS: usize = 72;

pub(in crate::editor) fn peak_vertical_bounds(
    peak: CachedPeak,
    center: f32,
    amplitude: f32,
) -> (f32, f32) {
    let top = center - peak.max.clamp(-1.0, 1.0) * amplitude;
    let bottom = center - peak.min.clamp(-1.0, 1.0) * amplitude;
    (top.min(bottom), top.max(bottom))
}

// Sample inside each fade as well as across the viewport, so a short curved fade
// on a long clip never collapses to a straight line between its endpoint knots.
fn fade_domain_positions(fade: ClipFade) -> impl Iterator<Item = u64> {
    // Invert the canonical two-axis law for drawing coordinates only. Uniform
    // gain spacing resolves steep bends; playback still uses gain_at directly.
    let at_gain = move |length: u64, index: usize, out: bool| {
        let fraction = fade
            .position_at_gain(out, index as f64 / 32.0)
            .unwrap_or(0.5);
        ((fraction * length.saturating_sub(1) as f64).round() as u64).min(length.saturating_sub(1))
    };
    let out_end = fade.span() - fade.out_end_offset();
    let midpoints = [
        (fade.in_curve() != 0 || fade.in_position() != 500)
            .then_some(fade.in_start() + fade.fade_in().saturating_sub(1) / 2),
        (fade.out_curve() != 0 || fade.out_position() != 500)
            .then_some(out_end - 1 - fade.fade_out().saturating_sub(1) / 2),
    ];
    fade.breakpoints()
        .into_iter()
        .flatten()
        .chain(midpoints.into_iter().flatten())
        .chain(
            (1..32)
                .flat_map(move |index| {
                    [
                        ((fade.in_curve() != 0 || fade.in_position() != 500) && fade.fade_in() > 1)
                            .then(|| fade.in_start() + at_gain(fade.fade_in(), index, false)),
                        ((fade.out_curve() != 0 || fade.out_position() != 500)
                            && fade.fade_out() > 1)
                            .then(|| {
                                out_end - fade.fade_out() + at_gain(fade.fade_out(), index, true)
                            }),
                    ]
                })
                .flatten(),
        )
}

/// Return a viewport-clipped, bounded preview of the canonical fade gain law.
/// X and gain are normalized to [0, 1]; sample phases stay in clip timeline
/// frames, independent of looping source-media phase.
pub(in crate::editor) fn projected_fade_envelope_samples(
    clip: &Clip,
    tempo: &hex_project::TempoMap,
    visible_start: u64,
    visible_end: u64,
) -> Result<Vec<(f32, f32)>, hex_project::Invalid> {
    let edit = clip
        .audio_edit()
        .ok_or(hex_project::Invalid("audio clip has no projected fade"))?;
    let fade = edit.projected_fade(tempo)?;
    let (_, _, view_start, _) = edit.projected_domain(tempo)?;
    if visible_start < clip.start()
        || visible_start >= visible_end
        || visible_end > clip.start().saturating_add(clip.duration())
    {
        return Ok(Vec::new());
    }
    let length = visible_end - visible_start;
    let count = length.min(FADE_ENVELOPE_SAMPLES as u64) as usize;
    let root_start = view_start
        .checked_add(visible_start - clip.start())
        .ok_or(hex_project::Invalid("fade geometry frame overflow"))?;
    let root_end = root_start
        .checked_add(length - 1)
        .ok_or(hex_project::Invalid("fade geometry frame overflow"))?;
    let capacity = count + 3 * FADE_ENVELOPE_BREAKPOINTS;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(capacity)
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for index in 0..count {
        positions.push(root_start + fraction_u64(length - 1, index, count.saturating_sub(1)));
    }
    for position in fade_domain_positions(clip.retained_fade()) {
        let Some(frame) = fade.frame_at_domain_position(position)? else {
            continue;
        };
        for frame in [frame.saturating_sub(1), frame, frame.saturating_add(1)] {
            if frame >= root_start && frame <= root_end {
                positions.push(frame);
            }
        }
    }
    positions.sort_unstable();
    positions.dedup();
    let mut result = Vec::new();
    result
        .try_reserve_exact(positions.len())
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for frame in positions {
        let x = if length == 1 {
            0.0
        } else {
            (frame - root_start) as f32 / (length - 1) as f32
        };
        result.push((x, fade.gain_at(frame)?));
    }
    Ok(result)
}

pub(in crate::editor) fn fade_envelope_samples(
    fade: ClipFade,
    clip_start: u64,
    visible_start: u64,
    visible_end: u64,
) -> Vec<(f32, f32)> {
    if fade.is_neutral() || visible_start < clip_start || visible_start >= visible_end {
        return Vec::new();
    }
    let visible_frames = visible_end - visible_start;
    let sample_count = usize::try_from(visible_frames.min(FADE_ENVELOPE_SAMPLES as u64))
        .unwrap_or(FADE_ENVELOPE_SAMPLES)
        .max(1);
    let Some(local_start) = visible_start.checked_sub(clip_start) else {
        return Vec::new();
    };
    let local_span = visible_frames.saturating_sub(1);
    let Some(local_end) = local_start.checked_add(local_span) else {
        return Vec::new();
    };
    let mut frames = Vec::with_capacity(sample_count + FADE_ENVELOPE_BREAKPOINTS);
    for index in 0..sample_count {
        frames.push(local_start.saturating_add(fraction_u64(
            local_span,
            index,
            sample_count.saturating_sub(1),
        )));
    }
    let mut add_domain_position = |position: u64| {
        if let Some(frame) = position.checked_sub(fade.offset())
            && frame >= local_start
            && frame <= local_end
        {
            frames.push(frame);
        }
    };
    for position in fade_domain_positions(fade) {
        add_domain_position(position);
    }
    frames.sort_unstable();
    frames.dedup();
    let mut points = Vec::with_capacity(frames.len());
    for frame in frames {
        let x = if local_span == 0 {
            0.0
        } else {
            (frame - local_start) as f32 / local_span as f32
        };
        points.push((x, fade.gain_at(frame).clamp(0.0, 1.0)));
    }
    if points.iter().all(|(_, gain)| *gain >= 1.0) {
        return Vec::new();
    }
    points
}

pub(in crate::editor) fn fraction_u64(value: u64, numerator: usize, denominator: usize) -> u64 {
    if denominator == 0 {
        return 0;
    }
    ((u128::from(value) * numerator as u128) / denominator as u128) as u64
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn waveform_peak_geometry_preserves_signed_channel_extrema() {
        let positive = peak_vertical_bounds(
            CachedPeak {
                min: 0.25,
                max: 0.75,
            },
            10.0,
            8.0,
        );
        assert!(positive.0 < positive.1 && positive.1 < 10.0);
        let negative = peak_vertical_bounds(
            CachedPeak {
                min: -0.75,
                max: -0.25,
            },
            10.0,
            8.0,
        );
        assert!(negative.0 > 10.0 && negative.1 > negative.0);
    }

    #[test]
    fn fade_envelope_samples_are_clipped_and_use_the_canonical_inherited_phase() {
        let fade = hex_project::ClipFade::new(1_000, 400, 600, 500).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 200, 240, 260);
        assert_eq!(samples.len(), 20);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().0, 0.0);
        assert_eq!(samples.last().unwrap().0, 1.0);
        assert_eq!(samples.first().unwrap().1, fade.gain_at(40));
        assert_eq!(samples.last().unwrap().1, fade.gain_at(59));
        assert!(fade_envelope_samples(fade, 200, 190, 220).is_empty());
        assert!(fade_envelope_samples(fade, 200, 240, 240).is_empty());
        let neutral = ClipFade::neutral(100).expect("neutral fade");
        assert!(fade_envelope_samples(neutral, 200, 200, 220).is_empty());
        let outside_fade = ClipFade::new(100, 40, 10, 10).expect("off-segment fade");
        assert!(fade_envelope_samples(outside_fade, 200, 220, 240).is_empty());
    }

    #[test]
    fn short_fade_breakpoint_stays_local_on_a_long_visible_clip() {
        let fade = ClipFade::new(1_000_000, 0, 241, 0).expect("short fade");
        let samples = fade_envelope_samples(fade, 0, 0, 1_000_000);
        assert!(samples.len() <= FADE_ENVELOPE_SAMPLES + FADE_ENVELOPE_BREAKPOINTS);
        assert_eq!(samples.first().unwrap().1, 0.0);
        let end = samples
            .iter()
            .find(|(x, _)| (*x - 240.0 / 999_999.0).abs() < f32::EPSILON)
            .expect("exact fade-in end breakpoint");
        assert_eq!(end.1, 1.0);
        let curved = fade.with_shape(0, 0, 800, 0).unwrap();
        let samples = fade_envelope_samples(curved, 0, 0, 1_000_000);
        let midpoint = samples
            .iter()
            .find(|(x, _)| (*x - 120.0 / 999_999.0).abs() < f32::EPSILON)
            .expect("short curve interior survives zoom out");
        assert_eq!(midpoint.1, curved.gain_at(120));
        assert!(midpoint.1 < 0.2);
        for (horizontal, knot) in [
            (fade.with_curve_positions(250, 500).unwrap(), 60),
            (
                ClipFade::new(1_000_000, 0, 0, 241)
                    .unwrap()
                    .with_curve_positions(500, 750)
                    .unwrap(),
                999_939,
            ),
        ] {
            let samples = fade_envelope_samples(horizontal, 0, 0, 1_000_000);
            let anchor = samples
                .iter()
                .find(|(x, _)| (*x - knot as f32 / 999_999.0).abs() < f32::EPSILON)
                .expect("horizontal-only curve anchor survives zoom out");
            assert_eq!(anchor.1, 0.5);
        }
    }

    #[test]
    fn inherited_fade_breakpoints_use_domain_positions_before_offset_translation() {
        let fade = ClipFade::new(1_000, 200, 250, 400).expect("inherited fade");
        let samples = fade_envelope_samples(fade, 1_000, 1_000, 1_800);
        let at_local_frame = |frame: u64| {
            samples
                .iter()
                .find(|(x, _)| (*x - frame as f32 / 799.0).abs() < f32::EPSILON)
                .copied()
                .expect("exact inherited breakpoint")
        };
        assert_eq!(at_local_frame(49).1, 1.0); // fade-in endpoint
        assert_eq!(at_local_frame(399).1, 1.0); // before fade-out
        assert_eq!(at_local_frame(400).1, 1.0); // fade-out start
        assert_eq!(at_local_frame(799).1, 0.0); // original span endpoint

        let fade_in_crop = fade_envelope_samples(fade, 1_000, 1_040, 1_060);
        assert!(
            fade_in_crop
                .iter()
                .any(|(x, gain)| { (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0 })
        );
        let fade_out_crop = fade_envelope_samples(fade, 1_000, 1_390, 1_410);
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 9.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
        assert!(
            fade_out_crop
                .iter()
                .any(|(x, gain)| (*x - 10.0 / 19.0).abs() < f32::EPSILON && *gain == 1.0)
        );
    }
}
