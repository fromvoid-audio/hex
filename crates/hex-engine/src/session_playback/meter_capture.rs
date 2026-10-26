//! Preallocated per-chunk accumulation of canonical post-fader output.
//! Capture contributions may arrive separately (including retained tails); the
//! endpoint is published only when the complete render transaction commits.
use super::{OutputCaptureId, SessionPlaybackError, capture::CaptureSink};
use crate::{Frame, PreparedRevision};

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
            for value in self.frames[..self.len].iter().flatten() {
                if value.is_finite() {
                    self.peak = self.peak.max(value.abs());
                }
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
    pub revision: &'a PreparedRevision,
    pub cells: &'a mut [Cell],
    pub downstream: Option<&'b mut dyn CaptureSink>,
    pub invalidated: bool,
}
impl MeterCapture<'_, '_> {
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
            if frames.len() > cell.frames.len() {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            // Contributions for one owner arrive in nondecreasing block
            // offsets. Keep its current span open so retained tails at the
            // same offset are summed before calculating the peak.
            if cell.offset.is_some_and(|previous| offset < previous) {
                // A broken ordering contract must not turn optional metering
                // into an audio failure or publish a misleading partial peak.
                cell.offset = None;
                cell.peak = 0.0;
                cell.discarded = true;
            }
            if !cell.discarded && cell.offset.is_some_and(|previous| offset > previous) {
                cell.finish();
            }
            if !cell.discarded && cell.offset.is_none() {
                cell.frames[..frames.len()].copy_from_slice(frames);
                cell.len = frames.len();
                cell.offset = Some(offset);
            } else if !cell.discarded {
                if frames.len() > cell.len {
                    cell.frames[cell.len..frames.len()].fill([0.0; 2]);
                    cell.len = frames.len();
                }
                for (destination, source) in cell.frames.iter_mut().zip(frames) {
                    destination[0] += source[0];
                    destination[1] += source[1];
                }
            }
        }
        if let Some(sink) = self.downstream.as_mut() {
            sink.add(id, offset, frames)?;
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
