//! Bounded worker-side retention of confirmed output packet timing.
//!
//! This index retains no PCM and does not establish common clock origin,
//! hardware latency or Arrangement placement authority.
use super::{End, FRAMES, OutputClock, Reader, TimingSpan};
use std::{
    fmt,
    ops::Range,
    sync::{Arc, atomic::AtomicU8},
};

const MAX_PACKETS: usize = 1_048_576;
const MAX_SPANS: usize = 1_048_576;

struct Packet {
    clock: OutputClock,
    frames: u32,
    spans: Range<usize>,
}

/// One retained confirmed packet. Empty timing spans explicitly represent
/// output frames for which no rendered Arrangement time was supplied.
pub struct OutputClockPacket<'a> {
    pub clock: OutputClock,
    pub frames: u32,
    pub timing: &'a [TimingSpan],
}

/// Bounded timing evidence from exactly one recorder reader. Poll and drop on
/// a non-audio worker. At most 1,048,576 packets and 1,048,576 spans are kept;
/// exhaustion stops collection and leaves the preceding evidence intact.
pub struct OutputClockIndex {
    state: Arc<AtomicU8>,
    rate: u32,
    source: Option<crate::prepared::AppliedAudioSource>,
    packets: Vec<Packet>,
    spans: Vec<TimingSpan>,
    terminal: Option<End>,
    failed: Option<OutputClockIndexError>,
    expected_drains: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputClockIndexError {
    WrongReader,
    ReaderConsumed,
    MissingClock,
    InvalidClock,
    InvalidTiming,
    Capacity,
    Allocation,
}
impl fmt::Display for OutputClockIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WrongReader => "output clock index belongs to another recording",
            Self::ReaderConsumed => "output recording was drained outside its clock index",
            Self::MissingClock => "confirmed output clock is unavailable",
            Self::InvalidClock => "output clock order or packet extent is invalid",
            Self::InvalidTiming => "rendered timeline timing is invalid",
            Self::Capacity => "output clock index capacity was exceeded",
            Self::Allocation => "could not reserve output clock index storage",
        })
    }
}
impl std::error::Error for OutputClockIndexError {}

impl OutputClockIndex {
    /// Bind to the original reader before it is moved to the worker. A prior
    /// or competing drain invalidates the index on its next poll. This does not
    /// start recording; the owner still calls `Reader::ready` and Stop.
    pub fn new(reader: &Reader) -> Self {
        Self {
            state: Arc::clone(&reader.state),
            rate: reader.rate,
            source: reader.source_binding(),
            packets: Vec::new(),
            spans: Vec::new(),
            terminal: None,
            failed: (reader.drain_count != 0).then_some(OutputClockIndexError::ReaderConsumed),
            expected_drains: reader.drain_count,
        }
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// A requested source, checked against the callback's actual source for each block.
    pub fn source_binding(&self) -> Option<crate::prepared::AppliedAudioSource> {
        self.source
    }
    pub fn len(&self) -> usize {
        self.packets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
    /// Collection failure or the terminal observed after all accepted packets.
    pub fn completion(&self) -> Result<Option<End>, OutputClockIndexError> {
        self.failed.map_or(Ok(self.terminal), Err)
    }

    #[cfg(test)]
    pub(crate) fn test_push(
        &mut self,
        frames: usize,
        timing: &[TimingSpan],
        clock: OutputClock,
    ) -> Result<(), OutputClockIndexError> {
        self.push(frames, timing, Some(clock), MAX_PACKETS, MAX_SPANS)
    }

    #[cfg(test)]
    pub(crate) fn test_finish(&mut self, terminal: End) {
        self.terminal = Some(terminal);
    }
    pub fn packet(&self, index: usize) -> Option<OutputClockPacket<'_>> {
        let packet = self.packets.get(index)?;
        Some(OutputClockPacket {
            clock: packet.clock,
            frames: packet.frames,
            timing: self.spans.get(packet.spans.clone())?,
        })
    }

    /// Drain at most the reader's bounded packet pool. An output terminal is
    /// returned only after the reader's accepted prefix. A collection error is
    /// latched and requests Stop; a wrong reader is refused without touching it.
    pub fn poll(&mut self, reader: &mut Reader) -> Result<Option<End>, OutputClockIndexError> {
        if !Arc::ptr_eq(&self.state, &reader.state) || self.rate != reader.rate {
            return Err(OutputClockIndexError::WrongReader);
        }
        if let Some(error) = self.failed {
            reader.stop();
            return Err(error);
        }
        if reader.drain_count != self.expected_drains || reader.drain_count == u64::MAX {
            reader.stop();
            self.failed = Some(OutputClockIndexError::ReaderConsumed);
            return Err(OutputClockIndexError::ReaderConsumed);
        }
        if let Some(terminal) = self.terminal {
            return Ok(Some(terminal));
        }
        let mut failure = None;
        let terminal = reader.drain_with_output_clock(|frames, _, _, timing, clock| {
            if failure.is_none() {
                failure = self
                    .push(frames, timing, clock, MAX_PACKETS, MAX_SPANS)
                    .err();
            }
        });
        self.expected_drains = reader.drain_count;
        if let Some(error) = failure {
            reader.stop();
            self.failed = Some(error);
            return Err(error);
        }
        self.terminal = terminal;
        Ok(terminal)
    }

    fn push(
        &mut self,
        frames: usize,
        timing: &[TimingSpan],
        clock: Option<OutputClock>,
        packet_limit: usize,
        span_limit: usize,
    ) -> Result<(), OutputClockIndexError> {
        let clock = clock.ok_or(OutputClockIndexError::MissingClock)?;
        let frames = u32::try_from(frames).map_err(|_| OutputClockIndexError::InvalidClock)?;
        if frames == 0
            || frames > FRAMES as u32
            || clock.offset_frames.checked_add(frames).is_none()
        {
            return Err(OutputClockIndexError::InvalidClock);
        }
        if let Some(previous) = self.packets.last()
            && (clock.playback < previous.clock.playback
                || (clock.playback == previous.clock.playback
                    && previous.clock.offset_frames.checked_add(previous.frames)
                        != Some(clock.offset_frames)))
        {
            return Err(OutputClockIndexError::InvalidClock);
        }
        let mut previous_end = 0;
        for span in timing {
            if span.take_start < previous_end
                || span.take_start >= span.take_end
                || span.take_end > u64::from(frames)
                || span
                    .timeline_start
                    .checked_add(span.take_end - span.take_start)
                    .is_none()
            {
                return Err(OutputClockIndexError::InvalidTiming);
            }
            previous_end = span.take_end;
        }
        let packet_count = self
            .packets
            .len()
            .checked_add(1)
            .ok_or(OutputClockIndexError::Capacity)?;
        let span_count = self
            .spans
            .len()
            .checked_add(timing.len())
            .ok_or(OutputClockIndexError::Capacity)?;
        reserve(&mut self.packets, packet_count, packet_limit)?;
        reserve(&mut self.spans, span_count, span_limit)?;
        let start = self.spans.len();
        self.spans.extend_from_slice(timing);
        self.packets.push(Packet {
            clock,
            frames,
            spans: start..span_count,
        });
        Ok(())
    }
}

fn reserve<T>(
    values: &mut Vec<T>,
    needed: usize,
    limit: usize,
) -> Result<(), OutputClockIndexError> {
    if needed > limit {
        return Err(OutputClockIndexError::Capacity);
    }
    if needed > values.capacity() {
        let target = needed
            .max(values.capacity().saturating_mul(2))
            .max(64)
            .min(limit);
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| OutputClockIndexError::Allocation)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::StreamInstant;

    fn reader() -> (super::super::Control, Reader) {
        let (mut control, _callback) = super::super::channel(48_000);
        let reader = control.start(1, &[], true).unwrap();
        (control, reader)
    }
    fn clock(nanos: u32, offset: u32) -> OutputClock {
        OutputClock {
            playback: StreamInstant::new(10, nanos),
            offset_frames: offset,
        }
    }
    fn timing(start: u64, end: u64, timeline: u64) -> TimingSpan {
        let project = hex_project::Project::new(48_000).unwrap();
        let map =
            hex_engine::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
        TimingSpan {
            take_start: start,
            take_end: end,
            timeline_start: timeline,
            tempo: map.capture_tempo(0).unwrap().0,
        }
    }

    #[test]
    fn wrong_reader_is_refused_without_stopping_or_poisoning_either_reader() {
        let (_a_control, mut a) = reader();
        let (_b_control, mut b) = reader();
        let mut index = OutputClockIndex::new(&a);
        assert_eq!(index.poll(&mut b), Err(OutputClockIndexError::WrongReader));
        assert!(b.ready());
        assert!(a.ready());
        assert_eq!(index.poll(&mut a).unwrap(), None);
    }

    #[test]
    fn competing_drains_are_detected_before_and_after_index_creation() {
        let (_control, mut already_read) = reader();
        already_read.drain(|_, _, _| {});
        let mut late = OutputClockIndex::new(&already_read);
        assert_eq!(
            late.poll(&mut already_read),
            Err(OutputClockIndexError::ReaderConsumed)
        );
        assert!(!already_read.ready());

        let (_control, mut active) = reader();
        let mut index = OutputClockIndex::new(&active);
        assert!(active.ready());
        assert_eq!(index.poll(&mut active).unwrap(), None);
        active.drain(|_, _, _| {});
        assert_eq!(
            index.poll(&mut active),
            Err(OutputClockIndexError::ReaderConsumed)
        );
        assert_eq!(
            index.poll(&mut active),
            Err(OutputClockIndexError::ReaderConsumed)
        );
    }

    #[test]
    fn validates_timing_and_clock_before_publishing_an_entry() {
        let (_control, reader) = reader();
        let mut index = OutputClockIndex::new(&reader);
        let good = timing(0, 3, 40);
        index.push(4, &[good], Some(clock(0, 0)), 4, 4).unwrap();
        assert_eq!(
            index.push(4, &[], None, 4, 4),
            Err(OutputClockIndexError::MissingClock)
        );
        assert_eq!(
            index.push(4, &[], Some(clock(0, 2)), 4, 4),
            Err(OutputClockIndexError::InvalidClock)
        );
        assert_eq!(
            index.push(4, &[timing(3, 5, 0)], Some(clock(0, 4)), 4, 4),
            Err(OutputClockIndexError::InvalidTiming)
        );
        assert_eq!(
            index.push(
                4,
                &[timing(2, 4, 0), timing(1, 2, 0)],
                Some(clock(0, 4)),
                4,
                4
            ),
            Err(OutputClockIndexError::InvalidTiming)
        );
        assert_eq!(
            index.push(4, &[timing(0, 2, u64::MAX)], Some(clock(0, 4)), 4, 4),
            Err(OutputClockIndexError::InvalidTiming)
        );
        assert_eq!(index.len(), 1);
        let retained = index.packet(0).unwrap();
        assert_eq!(retained.clock, clock(0, 0));
        assert_eq!(retained.frames, 4);
        assert_eq!(retained.timing, &[good]);
        index.push(4, &[], Some(clock(0, 4)), 4, 4).unwrap();
        assert!(index.packet(1).unwrap().timing.is_empty());
        assert_eq!(
            index.push(4, &[], Some(clock(0, 0)), 4, 4),
            Err(OutputClockIndexError::InvalidClock)
        );
    }

    #[test]
    fn capacity_failure_keeps_the_previous_prefix() {
        let (_control, reader) = reader();
        let mut index = OutputClockIndex::new(&reader);
        index
            .push(4, &[timing(0, 4, 10)], Some(clock(0, 0)), 1, 1)
            .unwrap();
        assert_eq!(
            index.push(4, &[timing(0, 4, 14)], Some(clock(0, 4)), 2, 1),
            Err(OutputClockIndexError::Capacity)
        );
        assert_eq!(
            index.push(4, &[], Some(clock(0, 4)), 1, 1),
            Err(OutputClockIndexError::Capacity)
        );
        assert_eq!(index.len(), 1);
        assert_eq!(index.packet(0).unwrap().timing[0].timeline_start, 10);
    }

    #[test]
    fn impossible_reservation_is_an_explicit_allocation_error() {
        let mut bytes = Vec::<u8>::new();
        assert_eq!(
            reserve(&mut bytes, usize::MAX, usize::MAX),
            Err(OutputClockIndexError::Allocation)
        );
        assert!(bytes.is_empty());
    }

    #[test]
    fn output_failure_is_returned_as_the_original_terminal() {
        let (mut control, mut callback) = super::super::channel(48_000);
        let mut reader = control.start(1, &[], true).unwrap();
        let mut index = OutputClockIndex::new(&reader);
        assert!(reader.ready());
        callback.begin(1);
        let (layout, stage) = callback.observation().unwrap();
        stage.observe(layout, hex_engine::ProjectSignal::Master, 0, &[[0.5, 0.5]]);
        callback.set_output_clock(Some(clock(0, 0)));
        callback.finish(false);
        assert_eq!(index.poll(&mut reader).unwrap(), Some(End::OutputFailed));
        assert!(index.is_empty());
    }

    #[test]
    fn missing_packet_clock_latches_failure_and_requests_stop() {
        let (mut control, mut callback) = super::super::channel(48_000);
        let mut reader = control.start(1, &[], true).unwrap();
        let mut index = OutputClockIndex::new(&reader);
        assert!(reader.ready());
        callback.begin(1);
        let (layout, stage) = callback.observation().unwrap();
        stage.observe(layout, hex_engine::ProjectSignal::Master, 0, &[[0.5, 0.5]]);
        callback.finish(true);
        assert_eq!(
            index.poll(&mut reader),
            Err(OutputClockIndexError::MissingClock)
        );
        assert_eq!(
            index.poll(&mut reader),
            Err(OutputClockIndexError::MissingClock)
        );
        callback.begin(1);
        assert_eq!(
            reader.drain(|_, _, _| panic!("unexpected packet")),
            Some(End::Stopped)
        );
    }
}
