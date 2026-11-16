//! Linear whole-take matching against retained output timing on control.
//!
//! The result is a raw candidate only. Source and clock provenance, graph and
//! hardware latency, and physical synchronization require separate evidence.
use super::timeline_overlap::frame_offset;
use super::{
    ClockOverlapError, InputTake, InputTakeEnd, OutputWindow, SharedClock, timeline_overlap,
};
use crate::sample_recording::{End, OutputClockIndex, OutputClockIndexError};
use std::fmt;

/// A contiguous rendered-timeline candidate, not project-edit authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidatePlacement {
    pub timeline_start: u64,
    pub frames: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeAlignmentError {
    InputNotStopped,
    OutputNotStopped,
    OutputEvidence(OutputClockIndexError),
    InvalidRate,
    RateMismatch,
    InvalidInput,
    InputClockGap,
    AmbiguousOutput,
    MissingCoverage,
    TimelineDiscontinuity,
    Clock(ClockOverlapError),
    Overflow,
}
impl fmt::Display for TakeAlignmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotStopped => f.write_str("input take did not stop normally"),
            Self::OutputNotStopped => f.write_str("output timing did not stop normally"),
            Self::OutputEvidence(error) => write!(f, "output timing is incomplete: {error}"),
            Self::InvalidRate => f.write_str("invalid recording sample rate"),
            Self::RateMismatch => f.write_str("input and output sample rates differ"),
            Self::InvalidInput => f.write_str("input timing does not describe the full take"),
            Self::InputClockGap => f.write_str("input clock is discontinuous"),
            Self::AmbiguousOutput => f.write_str("output packet clocks overlap or are invalid"),
            Self::MissingCoverage => f.write_str("output timeline does not cover the full take"),
            Self::TimelineDiscontinuity => f.write_str("rendered timeline is discontinuous"),
            Self::Clock(error) => write!(f, "clock overlap failed: {error}"),
            Self::Overflow => f.write_str("take alignment arithmetic overflowed"),
        }
    }
}
impl std::error::Error for TakeAlignmentError {}
impl From<ClockOverlapError> for TakeAlignmentError {
    fn from(error: ClockOverlapError) -> Self {
        Self::Clock(error)
    }
}

/// Match the entire normally stopped input take to a normally stopped output
/// index in linear time. The caller explicitly asserts a common clock origin.
/// Reject missing/overlapping clocks and any noncontiguous timeline mapping.
/// This does not prove the assertion, source identity, subframe drift, graph
/// latency, physical latency or permission to publish a project edit.
pub fn align_complete_take(
    shared: SharedClock,
    take: &InputTake,
    output: &OutputClockIndex,
) -> Result<CandidatePlacement, TakeAlignmentError> {
    if !matches!(take.end, InputTakeEnd::Stopped) || take.frames == 0 || take.open_error.is_some() {
        return Err(TakeAlignmentError::InputNotStopped);
    }
    match output
        .completion()
        .map_err(TakeAlignmentError::OutputEvidence)?
    {
        Some(End::Stopped) => {}
        _ => return Err(TakeAlignmentError::OutputNotStopped),
    }
    let rate = take.sample_rate;
    if !(8_000..=192_000).contains(&rate) || !(8_000..=192_000).contains(&output.sample_rate()) {
        return Err(TakeAlignmentError::InvalidRate);
    }
    if rate != output.sample_rate() {
        return Err(TakeAlignmentError::RateMismatch);
    }
    if take.timing.is_empty() || take.timing.len() > super::take::MAX_TIMING {
        return Err(TakeAlignmentError::InvalidInput);
    }
    let mut expected_take = 0_u64;
    let mut previous_input: Option<super::InputTiming> = None;
    for input in &take.timing {
        if input.frames == 0
            || input.take_start != expected_take
            || input
                .offset_frames
                .checked_add(input.frames)
                .is_none_or(|end| end > super::MAX_CALLBACK_FRAMES as u32)
        {
            return Err(TakeAlignmentError::InvalidInput);
        }
        expected_take = expected_take
            .checked_add(u64::from(input.frames))
            .ok_or(TakeAlignmentError::Overflow)?;
        if let Some(previous) = previous_input {
            let distance = frame_offset(
                rate,
                input.captured_at,
                u64::from(input.offset_frames),
                previous.captured_at,
                u64::from(previous.offset_frames),
            )?;
            if distance != i128::from(previous.frames) {
                return Err(TakeAlignmentError::InputClockGap);
            }
        }
        previous_input = Some(*input);
    }
    if expected_take != take.frames {
        return Err(TakeAlignmentError::InvalidInput);
    }

    // Reject overlapping packet clock ranges even when the rendered span in
    // one packet is missing: otherwise another packet could hide that gap.
    let mut previous_output: Option<(crate::sample_recording::OutputClock, u32)> = None;
    for i in 0..output.len() {
        let packet = output
            .packet(i)
            .ok_or(TakeAlignmentError::AmbiguousOutput)?;
        if let Some((previous, frames)) = previous_output {
            let distance = frame_offset(
                rate,
                packet.clock.playback,
                u64::from(packet.clock.offset_frames),
                previous.playback,
                u64::from(previous.offset_frames),
            )?;
            if distance < i128::from(frames) {
                return Err(TakeAlignmentError::AmbiguousOutput);
            }
        }
        previous_output = Some((packet.clock, packet.frames));
    }
    let mut windows = Windows {
        index: output,
        packet: 0,
        span: 0,
    };
    let mut window = windows.next()?;
    let mut timeline_start = None;
    let mut expected_timeline = 0_u64;
    for original in &take.timing {
        let mut input = *original;
        while input.frames != 0 {
            let current = window.ok_or(TakeAlignmentError::MissingCoverage)?;
            let output_offset =
                u64::from(current.clock.offset_frames) + u64::from(current.packet_start);
            let distance = frame_offset(
                rate,
                input.captured_at,
                u64::from(input.offset_frames),
                current.clock.playback,
                output_offset,
            )?;
            let window_frames = i128::from(current.packet_end - current.packet_start);
            if distance >= window_frames {
                window = windows.next()?;
                continue;
            }
            let overlap = timeline_overlap(shared, rate, output.sample_rate(), input, current)?
                .ok_or(TakeAlignmentError::MissingCoverage)?;
            if overlap.take_start != input.take_start {
                return Err(TakeAlignmentError::MissingCoverage);
            }
            if timeline_start.is_none() {
                timeline_start = Some(overlap.timeline_start);
                expected_timeline = overlap.timeline_start;
            }
            if overlap.timeline_start != expected_timeline {
                return Err(TakeAlignmentError::TimelineDiscontinuity);
            }
            expected_timeline = expected_timeline
                .checked_add(u64::from(overlap.frames))
                .ok_or(TakeAlignmentError::Overflow)?;
            input.take_start = input
                .take_start
                .checked_add(u64::from(overlap.frames))
                .ok_or(TakeAlignmentError::Overflow)?;
            input.offset_frames = input
                .offset_frames
                .checked_add(overlap.frames)
                .ok_or(TakeAlignmentError::Overflow)?;
            input.frames -= overlap.frames;
            if distance + i128::from(overlap.frames) >= window_frames {
                window = windows.next()?;
            }
        }
    }
    Ok(CandidatePlacement {
        timeline_start: timeline_start.ok_or(TakeAlignmentError::MissingCoverage)?,
        frames: take.frames,
    })
}

struct Windows<'a> {
    index: &'a OutputClockIndex,
    packet: usize,
    span: usize,
}
impl Windows<'_> {
    fn next(&mut self) -> Result<Option<OutputWindow>, TakeAlignmentError> {
        while let Some(packet) = self.index.packet(self.packet) {
            if let Some(span) = packet.timing.get(self.span) {
                self.span += 1;
                return Ok(Some(OutputWindow::from_recorded(
                    packet.clock,
                    packet.frames as usize,
                    *span,
                )?));
            }
            self.packet += 1;
            self.span = 0;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        input_capture::InputTiming,
        sample_recording::{OutputClock, TimingSpan},
    };
    use cpal::StreamInstant;
    use std::path::PathBuf;

    const RATE: u32 = 50_000;
    fn input(timing: Vec<InputTiming>, frames: u64) -> InputTake {
        InputTake {
            path: PathBuf::new(),
            sample_rate: RATE,
            frames,
            timing,
            end: InputTakeEnd::Stopped,
            open_error: None,
        }
    }
    fn segment(start: u64, frames: u32, nanos: u32, offset: u32) -> InputTiming {
        InputTiming {
            take_start: start,
            frames,
            captured_at: StreamInstant::new(10, nanos),
            offset_frames: offset,
        }
    }
    fn span(start: u64, end: u64, timeline: u64) -> TimingSpan {
        let project = hex_project::Project::new(RATE).unwrap();
        let map = hex_engine::PreparedTempoMap::prepare(project.tempo_map().clone(), RATE).unwrap();
        TimingSpan {
            take_start: start,
            take_end: end,
            timeline_start: timeline,
            tempo: map.capture_tempo(0).unwrap().0,
        }
    }
    fn output() -> OutputClockIndex {
        let (mut control, _callback) = crate::sample_recording::channel(RATE);
        let reader = control.start(1, &[], true).unwrap();
        OutputClockIndex::new(&reader)
    }
    fn push(
        index: &mut OutputClockIndex,
        nanos: u32,
        offset: u32,
        frames: usize,
        timing: &[TimingSpan],
    ) {
        index
            .test_push(
                frames,
                timing,
                OutputClock {
                    playback: StreamInstant::new(10, nanos),
                    offset_frames: offset,
                },
            )
            .unwrap();
    }
    fn completed(mut index: OutputClockIndex) -> OutputClockIndex {
        index.test_finish(End::Stopped);
        index
    }

    #[test]
    fn matches_split_packets_and_input_spans_as_one_contiguous_take() {
        let mut index = output();
        push(&mut index, 0, 0, 64, &[span(0, 32, 200), span(32, 64, 232)]);
        push(&mut index, 0, 64, 64, &[span(0, 64, 264)]);
        let take = input(vec![segment(0, 80, 0, 0), segment(80, 48, 0, 80)], 128);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(index)),
            Ok(CandidatePlacement {
                timeline_start: 200,
                frames: 128
            })
        );
    }

    #[test]
    fn half_frame_rounding_is_preserved_across_the_complete_take() {
        let mut index = output();
        push(&mut index, 0, 0, 64, &[span(0, 64, 200)]);
        let take = input(vec![segment(0, 63, 10_000, 0)], 63);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(index)),
            Ok(CandidatePlacement {
                timeline_start: 201,
                frames: 63
            })
        );
    }

    #[test]
    fn refuses_missing_ambiguous_and_discontinuous_output() {
        let take = input(vec![segment(0, 64, 0, 0)], 64);
        let mut gap = output();
        push(&mut gap, 0, 0, 64, &[span(0, 20, 0), span(21, 64, 21)]);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(gap)),
            Err(TakeAlignmentError::MissingCoverage)
        );
        let mut jumped = output();
        push(&mut jumped, 0, 0, 64, &[span(0, 32, 0), span(32, 64, 100)]);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(jumped)),
            Err(TakeAlignmentError::TimelineDiscontinuity)
        );
        let mut ambiguous = output();
        push(&mut ambiguous, 0, 0, 64, &[span(0, 64, 0)]);
        push(&mut ambiguous, 20_000, 0, 64, &[span(0, 64, 64)]);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(ambiguous)),
            Err(TakeAlignmentError::AmbiguousOutput)
        );
    }

    #[test]
    fn refuses_input_discontinuities_rates_and_incomplete_terminals() {
        let mut index = output();
        push(&mut index, 0, 0, 64, &[span(0, 64, 0)]);
        let mut take = input(vec![segment(0, 32, 0, 0), segment(32, 32, 0, 33)], 64);
        assert_eq!(
            align_complete_take(SharedClock, &take, &completed(index)),
            Err(TakeAlignmentError::InputClockGap)
        );
        take.timing[1].take_start = 33;
        let mut index = output();
        push(&mut index, 0, 0, 64, &[span(0, 64, 0)]);
        let index = completed(index);
        assert_eq!(
            align_complete_take(SharedClock, &take, &index),
            Err(TakeAlignmentError::InvalidInput)
        );
        take.timing[1].take_start = 32;
        take.sample_rate = 48_000;
        assert_eq!(
            align_complete_take(SharedClock, &take, &index),
            Err(TakeAlignmentError::RateMismatch)
        );
        take.sample_rate = RATE;
        take.end = InputTakeEnd::Capture(super::super::InputEnd::DeviceFailed);
        assert_eq!(
            align_complete_take(SharedClock, &take, &index),
            Err(TakeAlignmentError::InputNotStopped)
        );
        take.end = InputTakeEnd::Stopped;
        take.open_error = Some("device unavailable".into());
        assert_eq!(
            align_complete_take(SharedClock, &take, &index),
            Err(TakeAlignmentError::InputNotStopped)
        );
        take.open_error = None;
        let mut interrupted = output();
        interrupted.test_finish(End::OutputFailed);
        assert_eq!(
            align_complete_take(SharedClock, &take, &interrupted),
            Err(TakeAlignmentError::OutputNotStopped)
        );
    }

    #[test]
    fn scales_linearly_across_many_small_packets() {
        const COUNT: u32 = 8_000;
        let mut index = output();
        let mut timing = Vec::new();
        let tempo = span(0, 1, 0).tempo;
        for frame in 0..COUNT {
            index
                .test_push(
                    1,
                    &[TimingSpan {
                        take_start: 0,
                        take_end: 1,
                        timeline_start: u64::from(frame),
                        tempo,
                    }],
                    OutputClock {
                        playback: StreamInstant::new(10, 0),
                        offset_frames: frame,
                    },
                )
                .unwrap();
            timing.push(segment(u64::from(frame), 1, frame * 20_000, 0));
        }
        assert_eq!(
            align_complete_take(
                SharedClock,
                &input(timing, u64::from(COUNT)),
                &completed(index)
            ),
            Ok(CandidatePlacement {
                timeline_start: 0,
                frames: u64::from(COUNT)
            })
        );
    }
}
