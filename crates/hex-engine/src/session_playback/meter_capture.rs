//! Preallocated per-chunk accumulation of canonical post-fader output.
//! Capture contributions may arrive separately (including retained tails); the
//! endpoint is published only when the complete render transaction commits.
use super::{OutputCaptureId, SessionPlaybackError, capture::CaptureSink};
use crate::{Frame, PreparedRevision, signal_graph::Owner};
use hex_project::{MAX_RETURN_TRACKS, MAX_TRACKS};

pub(super) struct Cell {
    key: (u8, u64),
    frames: [Frame; 256],
    peak: Frame,
    offset: Option<usize>,
    len: usize,
    discarded: bool,
}
impl Cell {
    pub(super) fn new(id: OutputCaptureId) -> Self {
        Self {
            key: key(id),
            frames: [[0.0; 2]; 256],
            peak: [0.0; 2],
            offset: None,
            len: 0,
            discarded: false,
        }
    }
    fn finish(&mut self) {
        if self.offset.take().is_some() {
            if self.len == 1 {
                self.observe(self.frames[0]);
                return;
            }
            for frame in &self.frames[..self.len] {
                for (peak, value) in self.peak.iter_mut().zip(frame) {
                    if value.is_finite() {
                        *peak = peak.max(value.abs());
                    }
                }
            }
        }
    }

    fn add(&mut self, offset: usize, frames: &[Frame]) -> Result<(), SessionPlaybackError> {
        // Compensated routing contributes one frame at a time. Preserve the
        // same ordered-span and discard rules without the general slice loop.
        if let [frame] = frames {
            if self.offset.is_some_and(|previous| offset < previous) {
                self.offset = None;
                self.peak = [0.0; 2];
                self.discarded = true;
            }
            if self.discarded {
                return Ok(());
            }
            if self.offset.is_some_and(|previous| offset > previous) {
                self.finish();
            }
            if self.offset.is_none() {
                self.frames[0] = *frame;
                self.len = 1;
                self.offset = Some(offset);
            } else {
                if self.len == 0 {
                    self.frames[0] = [0.0; 2];
                    self.len = 1;
                }
                self.frames[0][0] += frame[0];
                self.frames[0][1] += frame[1];
            }
            return Ok(());
        }
        if frames.len() > self.frames.len() {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        // Contributions for one owner arrive in nondecreasing block
        // offsets. Keep its current span open so retained tails at the
        // same offset are summed before calculating the peak.
        if self.offset.is_some_and(|previous| offset < previous) {
            // A broken ordering contract must not turn optional metering
            // into an audio failure or publish a misleading partial peak.
            self.offset = None;
            self.peak = [0.0; 2];
            self.discarded = true;
        }
        if !self.discarded && self.offset.is_some_and(|previous| offset > previous) {
            self.finish();
        }
        if !self.discarded && self.offset.is_none() {
            if let [frame] = frames {
                self.frames[0] = *frame;
            } else {
                self.frames[..frames.len()].copy_from_slice(frames);
            }
            self.len = frames.len();
            self.offset = Some(offset);
        } else if !self.discarded {
            if frames.len() > self.len {
                self.frames[self.len..frames.len()].fill([0.0; 2]);
                self.len = frames.len();
            }
            for (destination, source) in self.frames.iter_mut().zip(frames) {
                destination[0] += source[0];
                destination[1] += source[1];
            }
        }
        Ok(())
    }

    #[inline]
    fn observe(&mut self, frame: Frame) {
        for (peak, value) in self.peak.iter_mut().zip(frame) {
            if value.is_finite() {
                *peak = peak.max(value.abs());
            }
        }
    }
}
fn key(id: OutputCaptureId) -> (u8, u64) {
    match id {
        OutputCaptureId::Track(id) => (0, id.value()),
        OutputCaptureId::Return(id) => (1, id.value()),
    }
}
pub(super) fn sort(cells: &mut [Cell]) {
    cells.sort_unstable_by_key(|cell| cell.key);
}

pub(super) struct MeterCapture<'a, 'b> {
    revision: &'a PreparedRevision,
    cells: &'a mut [Cell],
    downstream: Option<&'b mut dyn CaptureSink>,
    invalidated: bool,
    track_slots: [usize; MAX_TRACKS],
    return_slots: [usize; MAX_RETURN_TRACKS],
}
impl<'a, 'b> MeterCapture<'a, 'b> {
    pub(super) fn new(
        revision: &'a PreparedRevision,
        cells: &'a mut [Cell],
        downstream: Option<&'b mut dyn CaptureSink>,
    ) -> Self {
        let mut capture = Self {
            revision,
            cells,
            downstream,
            invalidated: false,
            track_slots: [usize::MAX; MAX_TRACKS],
            return_slots: [usize::MAX; MAX_RETURN_TRACKS],
        };
        let arrangement = revision.arrangement();
        for (track, index) in arrangement.tracks().iter().zip(0..MAX_TRACKS) {
            if let Some(slot) = track
                .canonical_track_id()
                .and_then(|id| capture.index(OutputCaptureId::Track(id)))
            {
                capture.track_slots[index] = slot;
            }
        }
        for (bus, index) in arrangement.return_buses().iter().zip(0..MAX_RETURN_TRACKS) {
            if let Some(slot) = capture.index(OutputCaptureId::Return(bus.id)) {
                capture.return_slots[index] = slot;
            }
        }
        capture
    }

    fn index(&self, id: OutputCaptureId) -> Option<usize> {
        self.cells
            .binary_search_by_key(&key(id), |cell| cell.key)
            .ok()
    }
    fn flush(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.finish();
        }
    }
}
impl CaptureSink for MeterCapture<'_, '_> {
    fn observes_timing(&self) -> bool {
        self.downstream
            .as_ref()
            .is_some_and(|sink| sink.observes_timing())
    }
    fn timing(&mut self, offset: usize, start: u64, frames: usize, map: &crate::PreparedTempoMap) {
        if let Some(sink) = self.downstream.as_mut() {
            sink.timing(offset, start, frames, map);
        }
    }
    fn selects(&self, id: OutputCaptureId) -> bool {
        self.index(id).is_some()
            || self
                .downstream
                .as_ref()
                .is_some_and(|sink| sink.selects(id))
    }
    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        if let Some(index) = self.index(id) {
            let cell = self
                .cells
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            cell.add(offset, frames)?;
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.add(id, offset, frames)?;
        }
        Ok(())
    }
    fn add_prepared_frame(
        &mut self,
        owner: Owner,
        id: OutputCaptureId,
        offset: usize,
        frame: Frame,
    ) -> Result<(), SessionPlaybackError> {
        let slot = match owner {
            Owner::Track(index) => self.track_slots.get(index),
            Owner::Return(index) => self.return_slots.get(index),
        };
        let cell = slot.and_then(|&slot| self.cells.get_mut(slot));
        if let Some(cell) = cell.filter(|cell| cell.key == key(id)) {
            cell.add(offset, &[frame])?;
        } else {
            return self.add(id, offset, &[frame]);
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.add(id, offset, &[frame])?;
        }
        Ok(())
    }

    fn commit(&mut self, frames: usize) {
        self.flush();
        let arrangement = self.revision.arrangement();
        for track in arrangement.tracks() {
            if let Some(index) = track
                .canonical_track_id()
                .and_then(|id| self.index(OutputCaptureId::Track(id)))
            {
                let cell = &mut self.cells[index];
                if !cell.discarded {
                    track.instrument_meter.observe_track_output(&[cell.peak]);
                }
                cell.peak = [0.0; 2];
                cell.discarded = false;
            }
        }
        for bus in arrangement.return_buses() {
            if let Some(index) = self.index(OutputCaptureId::Return(bus.id)) {
                let cell = &mut self.cells[index];
                if !cell.discarded {
                    bus.output_meter.observe_track_output(&[cell.peak]);
                }
                cell.peak = [0.0; 2];
                cell.discarded = false;
            }
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.commit(frames);
        }
    }
    fn invalidate(&mut self) {
        if self.invalidated {
            return;
        }
        self.invalidated = true;
        for cell in self.cells.iter_mut() {
            cell.peak = [0.0; 2];
            cell.offset = None;
            cell.discarded = false;
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Cell {
        Cell {
            key: (0, 1),
            frames: [[0.0; 2]; 256],
            peak: [0.0; 2],
            offset: None,
            len: 0,
            discarded: false,
        }
    }

    #[test]
    fn single_frame_contributions_keep_span_order_and_sum_before_peak() {
        let mut meter = cell();
        meter.add(0, &[]).unwrap();
        meter.add(0, &[[-0.5, 0.25]]).unwrap();
        meter.add(0, &[[0.25, 0.5], [0.75, -0.125]]).unwrap();
        meter.add(0, &[[0.125, -0.25]]).unwrap();
        meter.add(2, &[[f32::NAN, -0.5]]).unwrap();
        meter.add(3, &[[0.0, f32::INFINITY]]).unwrap();
        meter.finish();
        assert_eq!(meter.peak, [0.75, 0.5]);
        assert!(!meter.discarded);
    }

    #[test]
    fn single_frame_out_of_order_discards_prior_and_future_peaks() {
        let mut meter = cell();
        meter.add(2, &[[0.75, -0.5]]).unwrap();
        meter.add(3, &[[0.25, 0.25]]).unwrap();
        meter.add(1, &[[1.0, 1.0]]).unwrap();
        meter.add(4, &[[1.0, 1.0]]).unwrap();
        meter.finish();
        assert!(meter.discarded);
        assert_eq!(meter.peak, [0.0; 2]);
    }
}
