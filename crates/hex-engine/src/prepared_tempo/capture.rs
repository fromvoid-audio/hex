//! A scalar copy of one canonical tempo segment. The recorder can retain it
//! without holding a graph or allocating on the render thread.
use super::{BeatTime, PreparedTempoMap};

const HALF_FRAME: u128 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureTempo {
    beat_origin: BeatTime,
    frame_origin_q32: u128,
    micro_bpm: u32,
    sample_rate: u32,
}

impl PreparedTempoMap {
    /// Find the latest tempo boundary mapped to this frame, plus the first
    /// subsequent boundary. Equal-frame boundaries never consume a span.
    pub fn capture_tempo(&self, frame: u64) -> Option<(CaptureTempo, Option<u64>)> {
        let beat = self.frame_to_beat(frame).ok()?;
        let points = self.canonical.points();
        let index = points.partition_point(|point| point.beat() <= beat);
        let point = points.get(index.checked_sub(1)?)?;
        let next = points
            .get(index)
            .and_then(|point| self.beat_to_frame(point.beat()).ok());
        Some((
            CaptureTempo {
                beat_origin: point.beat(),
                frame_origin_q32: self.canonical.beat_to_frame_q32(point.beat()).ok()?,
                micro_bpm: point.tempo().micro_bpm(),
                sample_rate: self.sample_rate(),
            },
            next,
        ))
    }
}

impl CaptureTempo {
    /// Nearest-frame projection of an absolute Q32.32 beat within this segment.
    pub fn frame_at_beat(self, raw: u64) -> Option<u64> {
        let beat_delta = u128::from(raw.checked_sub(self.beat_origin.raw())?);
        let factor = 60_000_000_u128 * u128::from(self.sample_rate);
        let offset = beat_delta * factor / u128::from(self.micro_bpm);
        u64::try_from((self.frame_origin_q32 + offset + HALF_FRAME) >> 32).ok()
    }

    /// First absolute Q32.32 beat in this segment that projects to frame or later.
    pub fn beat_lower_bound(self, frame: u64) -> Option<u64> {
        let target = (u128::from(frame) << 32).saturating_sub(HALF_FRAME);
        let delta = target.saturating_sub(self.frame_origin_q32);
        let factor = 60_000_000_u128 * u128::from(self.sample_rate);
        let raw = (delta * u128::from(self.micro_bpm)).div_ceil(factor);
        u64::try_from(u128::from(self.beat_origin.raw()) + raw).ok()
    }
}
