//! Audible playhead: the Arrangement frame now leaving Main output.
//!
//! `Status::position` is the scheduled cursor, the next content frame the graph
//! renders. Main output trails it by the graph's Main latency L, so the frame
//! the user hears is the one scheduled L output frames ago. The callback keeps a
//! fixed history of rendered blocks and walks back L frames through it.
//!
//! Contract:
//! - Continuous playback: the scheduled position L output frames ago. A loop
//!   wrap inside a block maps through the content-clock loop law.
//! - Pause: frozen at the last audible frame (the performance clock stops).
//! - Seek, Play From and Stop: the performance clock jumps, history is
//!   discarded and the value is the new scheduled position. While fewer than L
//!   frames have played since, Main output is still silence after its reset
//!   and the value stays at the frame playback started from.
//! - History holds `CAPACITY` blocks. With blocks so short that L spans more,
//!   the value is the oldest retained block start: it lags by less than L.
//!
//! Bounded and allocation-free after control-side construction; recording and
//! scheduling keep using the scheduled position.

/// Enabled half-open content loop `(start, end)` in Arrangement frames.
type LoopFrames = (u64, u64);

/// Blocks retained. 1,024 blocks of 64 frames cover the 65,536-frame
/// compensation limit; ordinary latency needs one or two blocks.
const CAPACITY: usize = 1_024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Block {
    /// Scheduled Arrangement frame at block start.
    position: u64,
    /// Monotonic performance frame at block start.
    performance: u64,
    frames: u64,
}

impl Block {
    fn performance_end(self) -> Option<u64> {
        self.performance.checked_add(self.frames)
    }
}

pub(super) struct AudibleClock {
    blocks: Box<[Block]>,
    /// Index of the next slot to write.
    next: usize,
    len: usize,
}

impl AudibleClock {
    /// Control-side construction; the only allocation.
    pub(super) fn new() -> Self {
        Self {
            blocks: vec![Block::default(); CAPACITY].into_boxed_slice(),
            next: 0,
            len: 0,
        }
    }

    /// Record one rendered playing block that started at `position` and
    /// `performance`. A performance discontinuity discards the history.
    pub(super) fn push(&mut self, position: u64, performance: u64, frames: usize) {
        let Ok(frames) = u64::try_from(frames) else {
            self.len = 0;
            return;
        };
        if frames == 0 {
            return;
        }
        if self.newest().and_then(Block::performance_end) != Some(performance) {
            self.len = 0;
        }
        if let Some(slot) = self.blocks.get_mut(self.next) {
            *slot = Block {
                position,
                performance,
                frames,
            };
        }
        self.next = (self.next + 1) % self.blocks.len();
        self.len = (self.len + 1).min(self.blocks.len());
    }

    fn newest(&self) -> Option<Block> {
        if self.len == 0 {
            return None;
        }
        let index = (self.next + self.blocks.len() - 1) % self.blocks.len();
        self.blocks.get(index).copied()
    }

    /// Audible Arrangement frame for the current scheduled `position` and
    /// `performance` frame, `latency` output frames behind.
    pub(super) fn audible(
        &self,
        position: u64,
        performance: u64,
        latency: u32,
        region: Option<LoopFrames>,
    ) -> u64 {
        let Some(newest) = self.newest() else {
            return position;
        };
        if latency == 0 || newest.performance_end() != Some(performance) {
            return position;
        }
        let mut back = u64::from(latency);
        let mut oldest = newest;
        for age in 0..self.len {
            let index = (self.next + self.blocks.len() - 1 - age) % self.blocks.len();
            let Some(block) = self.blocks.get(index).copied() else {
                break;
            };
            oldest = block;
            if back <= block.frames {
                return content_frame(block.position, block.frames - back, region);
            }
            back -= block.frames;
        }
        oldest.position
    }
}

/// Content frame `offset` frames into a block that started at `start`.
fn content_frame(start: u64, offset: u64, region: Option<LoopFrames>) -> u64 {
    let Some(unwrapped) = start.checked_add(offset) else {
        return start;
    };
    match region.filter(|(loop_start, end)| loop_start < end && start < *end) {
        Some((loop_start, end)) if unwrapped >= end => {
            loop_start + (unwrapped - end) % (end - loop_start)
        }
        _ => unwrapped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a scheduled cursor through `frames` in blocks, wrapping at the
    /// loop end like the content clock, and return the audible value after
    /// each block together with the scheduled frame L earlier.
    fn play(
        clock: &mut AudibleClock,
        start: u64,
        frames: u64,
        block: u64,
        latency: u32,
        region: Option<LoopFrames>,
    ) -> (u64, u64) {
        let mut position = start;
        let mut performance = 0;
        let mut history = Vec::new();
        let step = |frame: u64| match region {
            Some((loop_start, end)) if frame + 1 == end => loop_start,
            _ => frame + 1,
        };
        let mut played = 0;
        while played < frames {
            let count = block.min(frames - played);
            clock.push(position, performance, count as usize);
            for _ in 0..count {
                history.push(position);
                position = step(position);
            }
            performance += count;
            played += count;
        }
        let audible = clock.audible(position, performance, latency, region);
        let expected = history
            .len()
            .checked_sub(latency as usize)
            .map_or(start, |index| *history.get(index).unwrap_or(&position));
        (audible, expected)
    }

    #[test]
    fn audible_trails_by_latency_across_wraps_at_several_block_sizes() {
        let region = Some((1_000, 1_500));
        for block in [1, 63, 256, 512, 1_024] {
            for frames in [0, 100, 288, 289, 700, 1_100, 3_333] {
                let mut clock = AudibleClock::new();
                let (audible, expected) = play(&mut clock, 900, frames, block, 288, region);
                assert_eq!(audible, expected, "block {block} frames {frames}");
            }
        }
    }

    #[test]
    fn seek_stop_and_pause_have_defined_values() {
        let mut clock = AudibleClock::new();
        clock.push(0, 0, 512);
        assert_eq!(clock.audible(512, 512, 288, None), 224);
        // Pause: the performance clock stops, so the value is frozen.
        assert_eq!(clock.audible(512, 512, 288, None), 224);
        // Seek before the next block: the new scheduled frame.
        assert_eq!(clock.audible(48_000, 48_000, 288, None), 48_000);
        // First block after the seek is shorter than L: still the seek target.
        clock.push(48_000, 48_000, 128);
        assert_eq!(clock.audible(48_128, 48_128, 288, None), 48_000);
        clock.push(48_128, 48_128, 128);
        assert_eq!(clock.audible(48_256, 48_256, 288, None), 48_000);
        clock.push(48_256, 48_256, 128);
        assert_eq!(clock.audible(48_384, 48_384, 288, None), 48_096);
        // No latency: audible is scheduled.
        assert_eq!(clock.audible(48_384, 48_384, 0, None), 48_384);
        // Stop rewinds both clocks to zero.
        assert_eq!(clock.audible(0, 0, 288, None), 0);
    }

    #[test]
    fn history_capacity_bounds_the_lookback() {
        let mut clock = AudibleClock::new();
        for index in 0..(CAPACITY as u64 + 10) {
            clock.push(index, index, 1);
        }
        let now = CAPACITY as u64 + 10;
        assert_eq!(clock.audible(now, now, 288, None), now - 288);
        // Beyond the retained blocks, the oldest retained start.
        assert_eq!(clock.audible(now, now, 60_000, None), 10);
    }
}
