//! Preallocated per-chunk accumulation of canonical post-fader output.
//! Capture contributions may arrive separately (including retained tails); the
//! endpoint is published only when the complete render transaction commits.
use super::{OutputCaptureId, SessionPlaybackError, capture::CaptureSink};
use crate::{Frame, PreparedRevision, signal_graph::Owner};
use hex_project::{MAX_RETURN_TRACKS, MAX_TRACKS};

pub(super) struct Cell {
    key: (u8, u64),
    frames: [Frame; 256],
    peak: f32,
    offset: Option<usize>,
    len: usize,
    discarded: bool,
}
impl Cell {
    pub(super) fn new(id: OutputCaptureId) -> Self {
        Self {
            key: key(id),
            frames: [[0.0; 2]; 256],
            peak: 0.0,
            offset: None,
            len: 0,
            discarded: false,
        }
    }
    fn finish(&mut self) {
        if self.offset.take().is_some() {
            if self.len == 1 {
                let [left, right] = self.frames[0];
                self.observe(left);
                self.observe(right);
                return;
            }
            for value in self.frames[..self.len].iter().flatten() {
                if value.is_finite() {
                    self.peak = self.peak.max(value.abs());
                }
            }
        }
    }

    fn add(&mut self, offset: usize, frames: &[Frame]) -> Result<(), SessionPlaybackError> {
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
            self.peak = 0.0;
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
    fn observe(&mut self, value: f32) {
        if value.is_finite() {
            self.peak = self.peak.max(value.abs());
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
                    track
                        .instrument_meter
                        .observe_track_output(&[[cell.peak, 0.0]]);
                }
                cell.peak = 0.0;
                cell.discarded = false;
            }
        }
        for bus in arrangement.return_buses() {
            if let Some(index) = self.index(OutputCaptureId::Return(bus.id)) {
                let cell = &mut self.cells[index];
                if !cell.discarded {
                    bus.output_meter.observe_track_output(&[[cell.peak, 0.0]]);
                }
                cell.peak = 0.0;
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
            cell.peak = 0.0;
            cell.offset = None;
            cell.discarded = false;
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.invalidate();
        }
    }
}
