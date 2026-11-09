//! Recorded guides and snap are projections of the same original take clock.
use super::selection::Range;
use hex_audio::sample_recording::TimingSpan;

const BEAT: u64 = 1 << 32;
const SNAP_STEP: u64 = BEAT / 4;
const MAX_GUIDES: usize = 256;

/// None means the frame has no recorded timing. A short known span can have
/// no grid point of its own; keep the raw frame instead of guessing a beat.
pub(super) fn snap(spans: &[TimingSpan], frame: u64) -> Option<u64> {
    let span = spans.get(spans.partition_point(|span| span.take_end <= frame))?;
    if frame < span.take_start {
        return None;
    }
    let beat = span.beat_lower_bound(frame)?;
    let upper = ceil_multiple(beat, SNAP_STEP)?;
    let lower = upper.checked_sub(SNAP_STEP);
    let snapped = [lower, Some(upper)]
        .into_iter()
        .flatten()
        .filter_map(|beat| span.frame_at_beat(beat))
        .min_by_key(|at| (frame.abs_diff(*at), std::cmp::Reverse(*at)));
    Some(snapped.unwrap_or(frame))
}

pub(super) fn visible(spans: &[TimingSpan], view: Range, rate: u32) -> Vec<u64> {
    let mut result = Vec::new();
    // Thin dense views, then cap discontinuous material with many short loops.
    let stride = (view.frames() / u64::from(rate).max(1) / 16).max(1);
    let step = stride.saturating_mul(BEAT);
    let start = spans.partition_point(|span| span.take_end <= view.start);
    for &span in spans[start..]
        .iter()
        .take_while(|span| span.take_start < view.end)
    {
        let Some(mut beat) = span
            .beat_lower_bound(view.start.max(span.take_start))
            .and_then(|raw| ceil_multiple(raw, step))
        else {
            continue;
        };
        while let Some(frame) = span.frame_at_beat(beat).filter(|frame| *frame < view.end) {
            result.push(frame);
            if result.len() == MAX_GUIDES {
                return result;
            }
            let Some(next) = beat.checked_add(step) else {
                break;
            };
            beat = next;
        }
    }
    result
}

fn ceil_multiple(value: u64, step: u64) -> Option<u64> {
    value.div_ceil(step).checked_mul(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};

    #[test]
    fn recorded_guides_and_snap_use_original_tempo_and_loop_origin() {
        let points = vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(BeatTime::from_raw(BEAT), Tempo::new(60_000_000).unwrap()),
        ];
        let map =
            hex_engine::PreparedTempoMap::prepare(TempoMap::new(48_000, points).unwrap(), 48_000)
                .unwrap();
        let first = map.capture_tempo(12_000).unwrap().0;
        let second = map.capture_tempo(24_000).unwrap().0;
        let spans = [
            TimingSpan {
                take_start: 500,
                take_end: 12_500,
                timeline_start: 12_000,
                tempo: first,
            },
            TimingSpan {
                take_start: 12_500,
                take_end: 60_501,
                timeline_start: 24_000,
                tempo: second,
            },
            TimingSpan {
                take_start: 60_501,
                take_end: 72_501,
                timeline_start: 12_000,
                tempo: first,
            },
        ];
        assert_eq!(
            visible(&spans, Range::full(72_501), 48_000),
            [12_500, 60_500]
        );
        assert_eq!(snap(&spans, 400), None);
        assert_eq!(snap(&spans, 4_000), Some(6_500));
        assert_eq!(snap(&spans, 21_000), Some(24_500));
        assert_eq!(snap(&spans, 63_000), Some(60_501));
    }
}
