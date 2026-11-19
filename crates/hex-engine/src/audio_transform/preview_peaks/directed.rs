//! Accepted musical content maps retained from the playback render pass.
use super::*;
use crate::audio_transform::warp_error;
use hex_project::WarpResampler;
use std::ops::Range;

const Q32: i128 = 1_i128 << 32;

/// A visible placement span of a full directed render map.
#[derive(Debug)]
pub struct PreparedDirectedEventSpan {
    destination: Range<u64>,
    offset: u64,
    map: hex_warp::TimeMap,
    _reservation: Reservation,
}

impl PreparedDirectedEventSpan {
    pub fn destination(&self) -> Range<u64> {
        self.destination.clone()
    }
}

/// Reflected or forward reads in the same contiguous base context that fed
/// the renderer. A crop retains the original map and its full-view offset.
/// The coordinates describe the center of the read, before interpolation.
#[derive(Debug)]
pub struct PreparedDirectedEventRead {
    base: Range<u64>,
    crop: Range<u64>,
    destination_start: u64,
    reversed: bool,
    law: WarpResampler,
    spans: Vec<PreparedDirectedEventSpan>,
    _reservation: Reservation,
}

impl PreparedDirectedEventRead {
    pub(in crate::audio_transform) fn admit(
        base: Range<u64>,
        crop: Range<u64>,
        destination_start: u64,
        reversed: bool,
        law: WarpResampler,
        count: u64,
        budget: &Arc<Budget>,
    ) -> Option<Self> {
        if base.is_empty() || crop.is_empty() {
            return None;
        }
        let count = usize::try_from(count).ok()?;
        let storage = count.checked_mul(std::mem::size_of::<PreparedDirectedEventSpan>())?;
        let reservation = budget.reserve(storage).ok()?;
        let mut spans = Vec::new();
        spans.try_reserve_exact(count).ok()?;
        Some(Self {
            base,
            crop,
            destination_start,
            reversed,
            law,
            spans,
            _reservation: reservation,
        })
    }

    pub(in crate::audio_transform) fn record(
        &mut self,
        full: Range<u64>,
        map: hex_warp::TimeMap,
        budget: &Arc<Budget>,
    ) -> Option<()> {
        let first = full.start.max(self.crop.start);
        let last = full.end.min(self.crop.end);
        if first >= last {
            return Some(());
        }
        let start = self
            .destination_start
            .checked_add(first - self.crop.start)?;
        let end = self.destination_start.checked_add(last - self.crop.start)?;
        let reservation = budget.reserve(map.backing_bytes()?).ok()?;
        self.spans.push(PreparedDirectedEventSpan {
            destination: start..end,
            offset: first - full.start,
            map,
            _reservation: reservation,
        });
        Some(())
    }

    pub(in crate::audio_transform) fn finish(&mut self) {
        if self.reversed {
            self.spans.reverse();
        }
    }

    /// Absolute virtual base frame interval used as the contiguous filter input.
    pub fn base(&self) -> Range<u64> {
        self.base.clone()
    }

    pub fn read_resampler(&self) -> WarpResampler {
        self.law
    }

    /// Audible map spans in ascending destination order.
    pub fn spans(&self) -> &[PreparedDirectedEventSpan] {
        &self.spans
    }

    /// Q64.32 position in the forward virtual base, the same frame clock
    /// as the occurrence entries. Outside the audible spans has no read.
    pub fn position_q32(&self, frame: u64) -> Result<Option<i128>, Invalid> {
        let index = self
            .spans
            .partition_point(|span| span.destination.end <= frame);
        let Some(span) = self
            .spans
            .get(index)
            .filter(|span| span.destination.contains(&frame))
        else {
            return Ok(None);
        };
        let local = i128::from(frame - span.destination.start) + i128::from(span.offset);
        let read = span.map.position_q32(local).map_err(warp_error)?;
        let forward = if self.reversed {
            i128::from(self.base.end - 1) * Q32 - read
        } else {
            i128::from(self.base.start) * Q32 + read
        };
        Ok(Some(forward))
    }
}
