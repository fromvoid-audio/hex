//! Bounded peak ordinates and fade sampling for waveform paint and previews.
use hex_project::{Clip, ClipFade};
use hex_session::CachedPeak;

const FADE_ENVELOPE_SAMPLES: usize = 32;
const FADE_ENVELOPE_BREAKPOINTS: usize = 72;
// Cap drawing work independently of a finite placement's repeat density.
const MAX_FADE_OCCURRENCES: usize = 32;
const MAX_PLACED_FADE_POINTS: usize =
    FADE_ENVELOPE_SAMPLES + MAX_FADE_OCCURRENCES * (2 + 3 * FADE_ENVELOPE_BREAKPOINTS);

/// Vertical amplitude mapping; independent of timeline and display scaling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::editor) enum WaveformAmplitudeScale {
    #[default]
    Linear,
    Decibel,
}

impl WaveformAmplitudeScale {
    pub(in crate::editor) fn from_decibels(enabled: bool) -> Self {
        if enabled { Self::Decibel } else { Self::Linear }
    }

    fn map(self, value: f32) -> f32 {
        match self {
            Self::Linear => value.clamp(-1.0, 1.0),
            Self::Decibel if value.is_finite() => {
                let value = value.clamp(-1.0, 1.0);
                value.signum() * ((99.0 * value.abs()).ln_1p() / 100.0_f32.ln()).min(1.0)
            }
            Self::Decibel => 0.0,
        }
    }
}

pub(in crate::editor) fn peak_vertical_bounds(
    peak: CachedPeak,
    center: f32,
    amplitude: f32,
    scale: WaveformAmplitudeScale,
) -> (f32, f32) {
    let top = center - scale.map(peak.max) * amplitude;
    let bottom = center - scale.map(peak.min) * amplitude;
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
        fade.ramp_is_curved(false)
            .then_some(fade.in_start() + fade.fade_in().saturating_sub(1) / 2),
        fade.ramp_is_curved(true)
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
                        (fade.ramp_is_curved(false) && fade.fade_in() > 1)
                            .then(|| fade.in_start() + at_gain(fade.fade_in(), index, false)),
                        (fade.ramp_is_curved(true) && fade.fade_out() > 1).then(|| {
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
    preview: Option<(ClipFade, bool)>,
) -> Result<Vec<(f32, f32)>, hex_project::Invalid> {
    let edit = clip
        .audio_edit()
        .ok_or(hex_project::Invalid("audio clip has no projected fade"))?;
    if visible_start < clip.start()
        || visible_start >= visible_end
        || visible_end > clip.start().saturating_add(clip.duration())
    {
        return Ok(Vec::new());
    }
    if let Some((fade, outer)) = preview {
        if !outer {
            return Ok(fade_envelope_samples(
                fade,
                clip.start(),
                visible_start,
                visible_end,
            ));
        }
        let projection = clip.projected_outer_fade_preview(tempo, fade)?;
        return outer_fade_samples(&projection, fade, visible_start, visible_end);
    }
    if let Some(projection) = clip.projected_outer_fade(tempo)? {
        return outer_fade_samples(
            &projection,
            clip.retained_fade(),
            visible_start,
            visible_end,
        );
    }
    if edit.placement.is_some()
        && edit.content_view.is_none()
        && matches!(edit.domain.clock, hex_project::AudioDomainClock::Frames(_))
    {
        return placed_frame_fade_samples(clip, tempo, visible_start, visible_end);
    }
    let fade = edit.projected_fade(tempo)?;
    let (_, _, view_start, _) = edit.projected_domain(tempo)?;
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

/// One bounded destination envelope. Repeat density never affects this pass.
fn outer_fade_samples(
    projection: &hex_project::ProjectedOuterAudioFade<'_>,
    fade: ClipFade,
    visible_start: u64,
    visible_end: u64,
) -> Result<Vec<(f32, f32)>, hex_project::Invalid> {
    if projection.is_neutral() {
        return Ok(Vec::new());
    }
    let length = visible_end - visible_start;
    let count = length.min(FADE_ENVELOPE_SAMPLES as u64) as usize;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(count + 3 * (FADE_ENVELOPE_BREAKPOINTS + 2))
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for index in 0..count {
        positions.push(visible_start + fraction_u64(length - 1, index, count.saturating_sub(1)));
    }
    for knot in fade_domain_positions(fade).chain([fade.offset(), fade.span()]) {
        if let Some(frame) = projection.frame_at_domain_position(knot)? {
            positions.extend(
                [frame.saturating_sub(1), frame, frame.saturating_add(1)]
                    .into_iter()
                    .filter(|frame| (visible_start..visible_end).contains(frame)),
            );
        }
    }
    positions.sort_unstable();
    positions.dedup();
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(positions.len())
        .map_err(|_| hex_project::Invalid("fade geometry allocation failed"))?;
    for frame in positions {
        let x = (frame - visible_start) as f32 / (length - 1).max(1) as f32;
        samples.push((x, projection.gain_at(frame)?));
    }
    if samples.iter().all(|(_, gain)| *gain == 1.0) {
        samples.clear();
    }
    Ok(samples)
}

/// Legacy frame placements retain their original per-occurrence fade law.
fn placed_frame_fade_samples(
    clip: &Clip,
    tempo: &hex_project::TempoMap,
    visible_start: u64,
    visible_end: u64,
) -> Result<Vec<(f32, f32)>, hex_project::Invalid> {
    use hex_project::{AudioViewProjection, Invalid};
    let edit = clip
        .audio_edit()
        .ok_or(Invalid("audio clip has no projected fade"))?;
    let projection = AudioViewProjection::new(clip, tempo)?;
    let fade = edit.projected_fade(tempo)?;
    let length = visible_end - visible_start;
    let count = length.min(FADE_ENVELOPE_SAMPLES as u64) as usize;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(count)
        .map_err(|_| Invalid("fade geometry allocation failed"))?;
    for index in 0..count {
        positions.push(visible_start + fraction_u64(length - 1, index, count.saturating_sub(1)));
    }
    // Binary search the visible occurrence interval; never walk hidden repeats.
    let mut first = 0;
    let mut end = projection.occurrence_count();
    while first < end {
        let mid = first + (end - first) / 2;
        if projection.occurrence(mid)?.destination_end <= visible_start {
            first = mid + 1;
        } else {
            end = mid;
        }
    }
    let mut last = first;
    end = projection.occurrence_count();
    while last < end {
        let mid = last + (end - last) / 2;
        if projection.occurrence(mid)?.destination_start < visible_end {
            last = mid + 1;
        } else {
            end = mid;
        }
    }
    let visible_count = last - first;
    let selected = visible_count.min(MAX_FADE_OCCURRENCES as u64) as usize;
    let mut knots = Vec::new();
    knots
        .try_reserve_exact(FADE_ENVELOPE_BREAKPOINTS)
        .map_err(|_| Invalid("fade geometry allocation failed"))?;
    for position in fade_domain_positions(clip.retained_fade()) {
        if let Some(root) = fade.frame_at_domain_position(position)? {
            knots.push(root);
        }
    }
    let extra = selected * (2 + 3 * knots.len());
    if extra > MAX_PLACED_FADE_POINTS - count {
        return Err(Invalid("fade geometry exceeds bounded points"));
    }
    positions
        .try_reserve_exact(extra)
        .map_err(|_| Invalid("fade geometry allocation failed"))?;
    for index in 0..selected {
        let ordinal = first + fraction_u64(visible_count - 1, index, selected.saturating_sub(1));
        let occurrence = projection.occurrence(ordinal)?;
        positions.push(occurrence.destination_start.max(visible_start));
        positions.push((occurrence.destination_end - 1).min(visible_end - 1));
        for &knot in &knots {
            for root in [knot.saturating_sub(1), knot, knot.saturating_add(1)] {
                if root < occurrence.root_start || root >= occurrence.root_end {
                    continue;
                }
                let frame = occurrence
                    .destination_start
                    .checked_add(root - occurrence.root_start)
                    .ok_or(Invalid("fade geometry frame overflow"))?;
                if frame >= visible_start && frame < visible_end {
                    positions.push(frame);
                }
            }
        }
    }
    positions.sort_unstable();
    positions.dedup();
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(positions.len())
        .map_err(|_| Invalid("fade geometry allocation failed"))?;
    for frame in positions {
        let gain = match projection.root_position_for_frame(frame)? {
            Some(root) => fade.gain_at(root)?,
            // No processing-root read (an exhausted Once or silent period) has
            // no authored envelope to shade.
            None => 1.0,
        };
        let x = if length == 1 {
            0.0
        } else {
            (frame - visible_start) as f32 / (length - 1) as f32
        };
        samples.push((x, gain));
    }
    Ok(samples)
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
    fn decibel_scale_preserves_endpoints_and_expands_quiet_signed_peaks() {
        let scale = WaveformAmplitudeScale::from_decibels(true);
        assert_eq!(scale.map(0.0), 0.0);
        assert!((scale.map(1.0) - 1.0).abs() <= f32::EPSILON);
        assert!((scale.map(-1.0) + 1.0).abs() <= f32::EPSILON);
        assert!(scale.map(0.1) > 0.1);
        assert_eq!(scale.map(-0.1), -scale.map(0.1));
        let mut previous = 0.0;
        for step in 1..=1_000 {
            let mapped = scale.map(step as f32 / 1_000.0);
            assert!(mapped >= previous && mapped <= 1.0);
            previous = mapped;
        }
        assert!(scale.map(f32::NAN).is_finite());
        assert!(scale.map(f32::INFINITY).is_finite());
    }

    #[test]
    fn waveform_peak_geometry_preserves_signed_channel_extrema() {
        let positive = peak_vertical_bounds(
            CachedPeak {
                min: 0.25,
                max: 0.75,
            },
            10.0,
            8.0,
            WaveformAmplitudeScale::Linear,
        );
        assert!(positive.0 < positive.1 && positive.1 < 10.0);
        let negative = peak_vertical_bounds(
            CachedPeak {
                min: -0.75,
                max: -0.25,
            },
            10.0,
            8.0,
            WaveformAmplitudeScale::Linear,
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
    fn repeated_fixture(root: u64, repeats: u64) -> (hex_project::Editor, hex_project::ClipId) {
        use hex_project::{
            AudioEventEdit, AudioEventFade, AudioViewResizeOperation, Command, Created, Editor,
            Project,
        };
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "original transient".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: root,
            })
            .unwrap()
        else {
            panic!("audio import")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        AudioEventFade::new(
                            0,
                            ClipFade::new(root, 0, (root / 8).min(301), (root / 4).min(3_787))
                                .unwrap(),
                        )
                        .unwrap(),
                    ),
                },
            })
            .unwrap();
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: root * repeats,
                },
            })
            .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 13_184.min(root),
                fade_out: 23_254.min(root * repeats),
            })
            .unwrap();
        (editor, clip)
    }

    #[test]
    fn repeated_frame_shade_uses_the_root_at_each_visible_cycle_and_stays_bounded() {
        let (editor, id) = repeated_fixture(19_174, 4);
        let clip = editor.project().clip(id).unwrap();
        let tempo = editor.project().tempo_map();
        let samples = projected_fade_envelope_samples(clip, tempo, 0, 76_696, None).unwrap();
        assert!(!samples.is_empty());
        assert!(samples.len() <= MAX_PLACED_FADE_POINTS);
        let gain_at = |frame: u64| {
            samples
                .iter()
                .find(|(x, _)| (*x - frame as f32 / 76_695.0).abs() < f32::EPSILON)
                .unwrap()
                .1
        };
        for cycle in 0..4 {
            assert_eq!(gain_at(cycle * 19_174), 0.0);
            assert_eq!(gain_at(cycle * 19_174 + 13_183), 1.0);
            assert_eq!(gain_at(cycle * 19_174 + 19_173), 1.0);
        }
        let (dense, id) = repeated_fixture(128, 1_024);
        let clip = dense.project().clip(id).unwrap();
        let samples = projected_fade_envelope_samples(
            clip,
            dense.project().tempo_map(),
            0,
            clip.duration(),
            None,
        )
        .unwrap();
        assert!(samples.len() <= MAX_PLACED_FADE_POINTS);
        assert!(samples.iter().any(|(_, gain)| *gain < 1.0));
    }

    #[test]
    fn repeated_frame_shade_preview_matches_committed_fade_after_phase_and_viewport_changes() {
        use hex_project::{AudioViewResizeOperation, Command};
        for phase in [0, 137] {
            let (mut editor, id) = repeated_fixture(19_174, 4);
            editor
                .execute(Command::ResizeAudioClipView {
                    clip: id,
                    operation: AudioViewResizeOperation::PlayStart { position: phase },
                })
                .unwrap();
            let preview = ClipFade::new(76_696, 0, 10_001, 20_003)
                .unwrap()
                .with_curve_laws([ClipFade::DEFAULT_S_CURVE; 2]);
            let windows = [(0, 76_696), (19_174 + 4_000, 2 * 19_174 + 17_000)];
            let held: Vec<_> = windows
                .iter()
                .map(|&(a, b)| {
                    projected_fade_envelope_samples(
                        editor.project().clip(id).unwrap(),
                        editor.project().tempo_map(),
                        a,
                        b,
                        Some((preview, true)),
                    )
                    .unwrap()
                })
                .collect();
            editor
                .execute(Command::SetClipFadeEnvelope {
                    clip: id,
                    fade: preview,
                })
                .unwrap();
            for ((a, b), expected) in windows.into_iter().zip(held) {
                let committed = projected_fade_envelope_samples(
                    editor.project().clip(id).unwrap(),
                    editor.project().tempo_map(),
                    a,
                    b,
                    None,
                )
                .unwrap();
                assert_eq!(committed, expected, "phase={phase} window={a}..{b}");
                if (a, b) == windows[0] {
                    assert_eq!(committed.first(), Some(&(0.0, 0.0)));
                    assert_eq!(committed.last(), Some(&(1.0, 0.0)));
                } else {
                    assert!(
                        committed.is_empty(),
                        "middle cycles have no outer fade shade"
                    );
                }
            }
        }
    }

    #[test]
    fn musical_repeat_shade_keeps_one_destination_envelope_through_tempo_and_reflection() {
        use hex_project::{
            AudioClipTiming, AudioViewProjection, AudioViewResizeOperation, BeatTime, Command,
            Tempo,
        };
        let (mut editor, id) = repeated_fixture(19_174, 4);
        editor
            .execute(Command::SetClipFades {
                clip: id,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ONE,
                tempo: Tempo::new(72_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipMusical {
                clip: id,
                enabled: true,
            })
            .unwrap();
        editor
            .execute(Command::ResizeAudioClipView {
                clip: id,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 82_000,
                },
            })
            .unwrap();
        let clip = editor.project().clip(id).unwrap();
        let end = clip.start() + clip.duration();
        let visible = (clip.start(), end);
        let fade = ClipFade::new(clip.duration(), 0, 24_000, 19_000)
            .unwrap()
            .with_curve_laws([ClipFade::DEFAULT_S_CURVE; 2]);
        let sample = |project: &hex_project::Project, preview| {
            projected_fade_envelope_samples(
                project.clip(id).unwrap(),
                project.tempo_map(),
                visible.0,
                visible.1,
                preview,
            )
            .unwrap()
        };
        let held = sample(editor.project(), Some((fade, true)));
        assert_eq!(held.first(), Some(&(0.0, 0.0)));
        assert_eq!(held.last(), Some(&(1.0, 0.0)));
        editor
            .execute(Command::SetClipFadeEnvelope { clip: id, fade })
            .unwrap();
        assert_eq!(sample(editor.project(), None), held);

        for command in [
            Command::SetAudioClipMusical {
                clip: id,
                enabled: false,
            },
            Command::SetTempoPoint {
                beat: BeatTime::ONE,
                tempo: Tempo::new(180_000_000).unwrap(),
            },
            Command::ReverseClips(vec![id]),
        ] {
            editor.execute(command).unwrap();
            assert_eq!(sample(editor.project(), None), held);
        }
        let project = editor.project();
        let clip = project.clip(id).unwrap();
        let edit = clip.audio_edit().unwrap();
        assert!(matches!(edit.timing, AudioClipTiming::Frozen { .. }));
        assert!(edit.content_view.unwrap().window.reversed);
        let view = AudioViewProjection::new(clip, project.tempo_map()).unwrap();
        let inside = view.occurrence(1).unwrap().destination_end;
        assert!(
            projected_fade_envelope_samples(clip, project.tempo_map(), inside, inside + 200, None)
                .unwrap()
                .is_empty(),
            "an interior repeat boundary must have no clip-level shading"
        );
    }
}
