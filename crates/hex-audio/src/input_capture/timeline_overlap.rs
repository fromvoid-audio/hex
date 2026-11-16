//! Checked arithmetic for one input/output clock-span overlap on control.
//!
//! The caller must establish shared clock provenance and source identity. This
//! calculation does not certify physical latency, graph latency, clock drift,
//! continuity across packets, or the safety of a project edit.
use super::{InputTiming, MAX_CALLBACK_FRAMES};
use crate::sample_recording::{OutputClock, TimingSpan};
use std::fmt;

const NANOSECONDS: i128 = 1_000_000_000;

/// The caller asserts that the two supplied timestamps use the same origin.
/// This value does not itself verify their devices, backend or clock domain.
#[derive(Clone, Copy, Debug)]
pub struct SharedClock;

/// One rendered timeline interval inside one confirmed output packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputWindow {
    pub clock: OutputClock,
    pub packet_frames: u32,
    pub packet_start: u32,
    pub packet_end: u32,
    pub timeline_start: u64,
}
impl OutputWindow {
    /// Convert one recorder timing span, refusing values that cannot be
    /// represented as packet-relative frame offsets. Full validation is done
    /// by `timeline_overlap`.
    pub fn from_recorded(
        clock: OutputClock,
        packet_frames: usize,
        span: TimingSpan,
    ) -> Result<Self, ClockOverlapError> {
        Ok(Self {
            clock,
            packet_frames: u32::try_from(packet_frames)
                .map_err(|_| ClockOverlapError::InvalidOutput)?,
            packet_start: u32::try_from(span.take_start)
                .map_err(|_| ClockOverlapError::InvalidOutput)?,
            packet_end: u32::try_from(span.take_end)
                .map_err(|_| ClockOverlapError::InvalidOutput)?,
            timeline_start: span.timeline_start,
        })
    }
}

/// A matched interval, expressed in input-take and rendered timeline frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineOverlap {
    pub take_start: u64,
    pub timeline_start: u64,
    pub frames: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockOverlapError {
    InvalidRate,
    RateMismatch,
    InvalidInput,
    InvalidOutput,
    TimeOutOfRange,
    Overflow,
}
impl fmt::Display for ClockOverlapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRate => "invalid input or output sample rate",
            Self::RateMismatch => "input and output sample rates differ",
            Self::InvalidInput => "invalid input timing span",
            Self::InvalidOutput => "invalid output timing span",
            Self::TimeOutOfRange => "clock difference is unavailable or too large",
            Self::Overflow => "clock or timeline arithmetic overflowed",
        })
    }
}
impl std::error::Error for ClockOverlapError {}

/// Intersect one input span with one output-render span on a caller-verified
/// shared clock. Both rates must match. Nanosecond and frame offsets are kept
/// as integer rational values until the final nearest-frame rounding (ties
/// toward the later frame). `None` means the two spans do not overlap. Each call is
/// independent: a consumer must detect gaps, ambiguity, discontinuities,
/// source changes and drift across the complete set before placing audio.
pub fn timeline_overlap(
    _shared_clock: SharedClock,
    input_rate: u32,
    output_rate: u32,
    input: InputTiming,
    output: OutputWindow,
) -> Result<Option<TimelineOverlap>, ClockOverlapError> {
    if !(8_000..=192_000).contains(&input_rate) || !(8_000..=192_000).contains(&output_rate) {
        return Err(ClockOverlapError::InvalidRate);
    }
    if input_rate != output_rate {
        return Err(ClockOverlapError::RateMismatch);
    }
    if input.frames == 0
        || input
            .offset_frames
            .checked_add(input.frames)
            .is_none_or(|end| end > MAX_CALLBACK_FRAMES as u32)
        || input
            .take_start
            .checked_add(u64::from(input.frames))
            .is_none()
    {
        return Err(ClockOverlapError::InvalidInput);
    }
    if output.packet_frames == 0
        || output.packet_frames > crate::sample_recording::FRAMES as u32
        || output.packet_start >= output.packet_end
        || output.packet_end > output.packet_frames
        || output
            .clock
            .offset_frames
            .checked_add(output.packet_frames)
            .is_none()
        || output
            .timeline_start
            .checked_add(u64::from(output.packet_end - output.packet_start))
            .is_none()
    {
        return Err(ClockOverlapError::InvalidOutput);
    }
    let (later, earlier, sign) = if input.captured_at >= output.clock.playback {
        (input.captured_at, output.clock.playback, 1_i128)
    } else {
        (output.clock.playback, input.captured_at, -1_i128)
    };
    let nanos = later
        .duration_since(&earlier)
        .ok_or(ClockOverlapError::TimeOutOfRange)?
        .as_nanos();
    let nanos = i128::try_from(nanos).map_err(|_| ClockOverlapError::TimeOutOfRange)?;
    let timestamp_ticks = nanos
        .checked_mul(i128::from(input_rate))
        .and_then(|v| v.checked_mul(sign))
        .ok_or(ClockOverlapError::Overflow)?;
    let output_offset = u64::from(output.clock.offset_frames) + u64::from(output.packet_start);
    let frame_delta = i128::from(input.offset_frames) - i128::from(output_offset);
    let ticks = timestamp_ticks
        .checked_add(
            frame_delta
                .checked_mul(NANOSECONDS)
                .ok_or(ClockOverlapError::Overflow)?,
        )
        .ok_or(ClockOverlapError::Overflow)?;
    let offset = ticks
        .checked_add(NANOSECONDS / 2)
        .ok_or(ClockOverlapError::Overflow)?
        .div_euclid(NANOSECONDS);
    let input_frames = i128::from(input.frames);
    let output_frames = i128::from(output.packet_end - output.packet_start);
    let begin = 0_i128.max(-offset);
    let end = input_frames.min(output_frames - offset);
    if begin >= end {
        return Ok(None);
    }
    let take_delta = u64::try_from(begin).map_err(|_| ClockOverlapError::Overflow)?;
    let timeline_delta = u64::try_from(offset + begin).map_err(|_| ClockOverlapError::Overflow)?;
    let frames = u32::try_from(end - begin).map_err(|_| ClockOverlapError::Overflow)?;
    Ok(Some(TimelineOverlap {
        take_start: input
            .take_start
            .checked_add(take_delta)
            .ok_or(ClockOverlapError::Overflow)?,
        timeline_start: output
            .timeline_start
            .checked_add(timeline_delta)
            .ok_or(ClockOverlapError::Overflow)?,
        frames,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::StreamInstant;

    fn input(nanos: u32, offset: u32, frames: u32) -> InputTiming {
        InputTiming {
            take_start: 100,
            frames,
            captured_at: StreamInstant::new(10, nanos),
            offset_frames: offset,
        }
    }
    fn output(nanos: u32, native_offset: u32, start: u32, end: u32, timeline: u64) -> OutputWindow {
        OutputWindow {
            clock: OutputClock {
                playback: StreamInstant::new(10, nanos),
                offset_frames: native_offset,
            },
            packet_frames: 256,
            packet_start: start,
            packet_end: end,
            timeline_start: timeline,
        }
    }
    fn overlap(
        input: InputTiming,
        output: OutputWindow,
    ) -> Result<Option<TimelineOverlap>, ClockOverlapError> {
        timeline_overlap(SharedClock, 48_000, 48_000, input, output)
    }

    #[test]
    fn intersects_both_sides_and_applies_native_and_packet_offsets() {
        let out = output(0, 100, 20, 80, 1_000);
        assert_eq!(
            overlap(input(0, 110, 30), out).unwrap(),
            Some(TimelineOverlap {
                take_start: 110,
                timeline_start: 1_000,
                frames: 20
            })
        );
        assert_eq!(
            overlap(input(0, 170, 30), out).unwrap(),
            Some(TimelineOverlap {
                take_start: 100,
                timeline_start: 1_050,
                frames: 10
            })
        );
        assert_eq!(overlap(input(0, 180, 10), out).unwrap(), None);
        assert_eq!(overlap(input(0, 0, 120), out).unwrap(), None);
    }

    #[test]
    fn rounds_subframe_times_without_floating_point() {
        let out = output(0, 0, 0, 20, 1_000);
        assert_eq!(
            overlap(input(10_416, 0, 5), out)
                .unwrap()
                .unwrap()
                .timeline_start,
            1_000
        );
        assert_eq!(
            overlap(input(10_417, 0, 5), out)
                .unwrap()
                .unwrap()
                .timeline_start,
            1_001
        );
        let negative = output(10_417, 0, 0, 20, 1_000);
        assert_eq!(
            overlap(input(0, 0, 5), negative).unwrap(),
            Some(TimelineOverlap {
                take_start: 101,
                timeline_start: 1_000,
                frames: 4
            })
        );
        let before = output(0, 0, 0, 20, 1_000);
        let mut earlier = input(999_980_000, 0, 5);
        earlier.captured_at = StreamInstant::new(9, 999_980_000);
        assert_eq!(
            overlap(earlier, before).unwrap(),
            Some(TimelineOverlap {
                take_start: 101,
                timeline_start: 1_000,
                frames: 4
            })
        );
    }

    #[test]
    fn exact_half_frames_round_toward_the_later_frame() {
        let out = output(0, 0, 0, 20, 1_000);
        let later = timeline_overlap(SharedClock, 50_000, 50_000, input(10_000, 0, 5), out)
            .unwrap()
            .unwrap();
        assert_eq!(later.timeline_start, 1_001);
        let earlier_out = output(10_000, 0, 0, 20, 1_000);
        let earlier = timeline_overlap(SharedClock, 50_000, 50_000, input(0, 0, 5), earlier_out)
            .unwrap()
            .unwrap();
        assert_eq!(
            (earlier.take_start, earlier.timeline_start, earlier.frames),
            (100, 1_000, 5)
        );
    }

    #[test]
    fn different_timeline_positions_do_not_imply_continuity() {
        let first = overlap(input(0, 0, 8), output(0, 0, 0, 8, 3_000))
            .unwrap()
            .unwrap();
        let looped = overlap(input(0, 8, 8), output(0, 8, 0, 8, 20))
            .unwrap()
            .unwrap();
        assert_eq!((first.timeline_start, looped.timeline_start), (3_000, 20));
    }

    #[test]
    fn recorded_timing_span_retains_its_packet_offsets() {
        let project = hex_project::Project::new(48_000).unwrap();
        let map =
            hex_engine::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
        let tempo = map.capture_tempo(0).unwrap().0;
        let clock = output(0, 100, 0, 1, 0).clock;
        let span = TimingSpan {
            take_start: 20,
            take_end: 80,
            timeline_start: 1_000,
            tempo,
        };
        let window = OutputWindow::from_recorded(clock, 256, span).unwrap();
        assert_eq!(window, output(0, 100, 20, 80, 1_000));
        assert_eq!(
            overlap(input(0, 110, 30), window).unwrap(),
            Some(TimelineOverlap {
                take_start: 110,
                timeline_start: 1_000,
                frames: 20
            })
        );
    }

    #[test]
    fn rejects_unusable_rates_extents_and_unrepresentable_times() {
        let good_in = input(0, 0, 8);
        let good_out = output(0, 0, 0, 8, 10);
        assert_eq!(
            timeline_overlap(SharedClock, 0, 48_000, good_in, good_out),
            Err(ClockOverlapError::InvalidRate)
        );
        assert_eq!(
            timeline_overlap(SharedClock, 44_100, 48_000, good_in, good_out),
            Err(ClockOverlapError::RateMismatch)
        );
        assert_eq!(
            overlap(input(0, 0, 0), good_out),
            Err(ClockOverlapError::InvalidInput)
        );
        let mut invalid = good_in;
        invalid.take_start = u64::MAX;
        assert_eq!(
            overlap(invalid, good_out),
            Err(ClockOverlapError::InvalidInput)
        );
        let mut invalid = good_out;
        invalid.packet_end = 257;
        assert_eq!(
            overlap(good_in, invalid),
            Err(ClockOverlapError::InvalidOutput)
        );
        invalid = good_out;
        invalid.timeline_start = u64::MAX;
        assert_eq!(
            overlap(good_in, invalid),
            Err(ClockOverlapError::InvalidOutput)
        );
        invalid = good_out;
        invalid.clock.offset_frames = u32::MAX;
        assert_eq!(
            overlap(good_in, invalid),
            Err(ClockOverlapError::InvalidOutput)
        );
        let mut distant = good_in;
        distant.captured_at = StreamInstant::new(i64::MAX, 0);
        assert_eq!(
            overlap(distant, good_out),
            Err(ClockOverlapError::TimeOutOfRange)
        );
    }
}
