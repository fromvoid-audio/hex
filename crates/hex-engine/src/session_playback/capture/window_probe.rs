//! Test-only export for measuring direct prepared track-window custody.
use super::{CaptureSink, OutputCaptureId, SessionPlaybackError};
use crate::{Frame, signal_graph::Owner};

pub(in crate::session_playback) struct TrackWindowProbe<'a, const N: usize> {
    pub(in crate::session_playback) ids: &'a [u64],
    pub(in crate::session_playback) tracks: &'a mut [[Frame; N]],
}

impl<const N: usize> TrackWindowProbe<'_, N> {
    fn store(
        &mut self,
        slot: usize,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        let end = offset
            .checked_add(frames.len())
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        self.tracks
            .get_mut(slot)
            .and_then(|track| track.get_mut(offset..end))
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .copy_from_slice(frames);
        Ok(())
    }
}

impl<const N: usize> CaptureSink for TrackWindowProbe<'_, N> {
    fn selects(&self, id: OutputCaptureId) -> bool {
        matches!(id, OutputCaptureId::Track(track) if self.ids.contains(&track.value()))
    }

    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        if let OutputCaptureId::Track(track) = id
            && let Some(slot) = self.ids.iter().position(|id| *id == track.value())
        {
            self.store(slot, offset, frames)?;
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
        if let (Owner::Track(slot), OutputCaptureId::Track(track)) = (owner, id) {
            if self.ids.get(slot) != Some(&track.value()) {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            self.store(slot, offset, &[frame])?;
        }
        Ok(())
    }

    fn invalidate(&mut self) {
        self.tracks.fill([[0.0; 2]; N]);
    }
}
