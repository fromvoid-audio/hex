//! Recorded clock spans accompany confirmed PCM packets. The worker coalesces
//! adjacent transport frames at the same tempo. Capacity exhaustion leaves
//! unknown timing without shortening or interrupting the recorded audio.
use hex_engine::CaptureTempo;

pub(super) const MAX_SPANS: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingSpan {
    pub take_start: u64,
    pub take_end: u64,
    pub timeline_start: u64,
    pub tempo: CaptureTempo,
}

impl TimingSpan {
    /// First beat in this clock segment at or beyond the specified take frame.
    pub fn beat_lower_bound(self, frame: u64) -> Option<u64> {
        let timeline = self
            .timeline_start
            .checked_add(frame.checked_sub(self.take_start)?)?;
        self.tempo.beat_lower_bound(timeline)
    }

    pub fn frame_at_beat(self, beat: u64) -> Option<u64> {
        let offset = self
            .tempo
            .frame_at_beat(beat)?
            .checked_sub(self.timeline_start)?;
        let frame = self.take_start.checked_add(offset)?;
        (frame < self.take_end).then_some(frame)
    }
}

pub(super) fn append(spans: &mut Vec<TimingSpan>, span: TimingSpan, limit: usize) {
    if let Some(last) = spans.last_mut()
        && last.take_end == span.take_start
        && last
            .timeline_start
            .checked_add(last.take_end - last.take_start)
            == Some(span.timeline_start)
        && last.tempo == span.tempo
    {
        last.take_end = span.take_end;
    } else if spans.len() < limit {
        spans.push(span);
    }
}
