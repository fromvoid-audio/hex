//! Bounded control plans and borrowed, transactional canonical output buffers.
use super::{SessionPlayback, SessionPlaybackError};
use crate::{Frame, PreparedRevision};
use hex_project::{ReturnId, TrackId};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Selection bound, independent of the number of retained historical owners.
pub const MAX_OUTPUT_CAPTURES: usize = hex_project::MAX_TRACKS + hex_project::MAX_RETURN_TRACKS;
/// Maximum complete transaction length; internal DSP chunks remain 256 frames.
pub const MAX_OUTPUT_CAPTURE_FRAMES: usize = 16_384;

/// An original canonical owner, never a current track/bus array index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputCaptureId {
    Track(TrackId),
    Return(ReturnId),
}

/// Actual direct post-device/post-fader track output, or Main including Returns
/// and master gain. Monitor-only metronome/browser audio is outside this tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectSignal {
    Track(u64),
    Master,
    /// This runtime cannot supply the requested current canonical owners.
    Unavailable,
}

/// Speculative synchronous samples. Publish only after complete rendering and
/// final caller output/context validation. Real-time observers must not allocate,
/// block, panic, retain the borrow or destroy an owner.
pub type ProjectSignalObserver<'observer> =
    dyn for<'frames> FnMut(ProjectSignal, usize, &'frames [Frame]) + 'observer;

/// Borrowed, bounded live selection. Unlike the offline capture plan, this does
/// not change scheduling or restrict loops/Session launches. IDs must be sorted,
/// distinct canonical TrackIds; callers validate them against the active source.
pub struct ProjectOutputTap<'a, 'observer> {
    tracks: &'a [u64],
    observer: &'a mut ProjectSignalObserver<'observer>,
    // Callback-relative start of the current render part. `Playback` splits a
    // callback only at a transport declick boundary and restores zero after.
    part_offset: usize,
}
impl<'a, 'observer> ProjectOutputTap<'a, 'observer> {
    pub fn tracks(&self) -> &[u64] {
        self.tracks
    }
    /// Forward a speculative borrowed span through a chunk-offset adapter.
    pub fn observe(&mut self, signal: ProjectSignal, offset: usize, frames: &[Frame]) {
        (self.observer)(signal, offset, frames);
    }
    pub fn new(
        tracks: &'a [u64],
        observer: &'a mut ProjectSignalObserver<'observer>,
    ) -> Option<Self> {
        (tracks.len() <= hex_project::MAX_TRACKS
            && tracks.iter().all(|id| *id != 0)
            && tracks.windows(2).all(|pair| pair[0] < pair[1]))
        .then_some(Self {
            tracks,
            observer,
            part_offset: 0,
        })
    }
    pub(crate) fn set_part_offset(&mut self, offset: usize) {
        self.part_offset = offset;
    }
    pub(crate) fn main(&mut self, frames: &[Frame]) {
        (self.observer)(ProjectSignal::Master, self.part_offset, frames);
    }
    pub(crate) fn unavailable(&mut self) {
        (self.observer)(ProjectSignal::Unavailable, 0, &[]);
    }
}

pub(super) trait CaptureSink {
    fn selects(&self, id: OutputCaptureId) -> bool;
    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError>;
    fn commit(&mut self, _frames: usize) {}
    fn invalidate(&mut self) {}
}
impl CaptureSink for ProjectOutputTap<'_, '_> {
    fn selects(&self, id: OutputCaptureId) -> bool {
        matches!(id, OutputCaptureId::Track(id) if self.tracks.binary_search(&id.value()).is_ok())
    }
    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        if let OutputCaptureId::Track(id) = id
            && self.selects(OutputCaptureId::Track(id))
        {
            (self.observer)(
                ProjectSignal::Track(id.value()),
                self.part_offset.saturating_add(offset),
                frames,
            );
        }
        Ok(())
    }
}

impl CaptureSink for OutputCapture<'_, '_> {
    fn selects(&self, id: OutputCaptureId) -> bool {
        OutputCapture::selects(self, id)
    }
    fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        OutputCapture::add(self, id, offset, frames)
    }
    fn commit(&mut self, frames: usize) {
        OutputCapture::commit(self, frames);
    }
    fn invalidate(&mut self) {
        OutputCapture::invalidate(self);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputCaptureError {
    UnsupportedRuntime,
    TooManyOutputs,
    DuplicateOwner,
    MissingOwner,
    StalePlan,
    InsufficientCapacity,
    Allocation,
    GenerationExhausted,
}

impl OutputCaptureError {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::UnsupportedRuntime => {
                "output capture requires an arrangement-only cut without transport loops or pending launches"
            }
            Self::TooManyOutputs => "output capture selection exceeds its bound",
            Self::DuplicateOwner => "output capture owner is duplicated",
            Self::MissingOwner => "output capture owner is not retained by this runtime",
            Self::StalePlan => "output capture plan belongs to another runtime generation",
            Self::InsufficientCapacity => "output capture buffers have insufficient capacity",
            Self::Allocation => "could not reserve output capture plan storage",
            Self::GenerationExhausted => "output capture runtime generation exhausted",
        }
    }
}

impl fmt::Display for OutputCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for OutputCaptureError {}

// Only construction on control touches this counter; render never does.
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);
pub(super) fn next_runtime() -> Result<u64, SessionPlaybackError> {
    NEXT_RUNTIME
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| SessionPlaybackError::Capture(OutputCaptureError::GenerationExhausted))
}

/// Built on control against one exact runtime, after any history inheritance.
/// Request order is buffer order. Retired-only IDs are explicitly selectable
/// while represented by that runtime's retained owner registry. Publication
/// requires a new plan, even when the prepared revision Arc is unchanged.
#[derive(Debug)]
pub struct OutputCapturePlan {
    revision: Arc<PreparedRevision>,
    runtime: u64,
    epoch: u64,
    ids: Vec<OutputCaptureId>,
    frame_capacity: usize,
}

impl OutputCapturePlan {
    pub fn ids(&self) -> &[OutputCaptureId] {
        &self.ids
    }
    pub fn frame_capacity(&self) -> usize {
        self.frame_capacity
    }

    /// Actual owned plan headers/capacity, excluding the shared revision.
    pub fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.ids.capacity() * std::mem::size_of::<OutputCaptureId>()
    }
}

/// Borrowed preallocated stereo frames, one slice per plan ID. Only the prefix
/// `valid_frames()` is committed, and only after the entire render succeeds.
/// Every refusal invalidates the entire view and clears within the fixed output
/// and frame bounds. Oversized invalid views never cause an unbounded scan.
pub struct OutputCapture<'a, 'samples> {
    pub(super) plan: &'a OutputCapturePlan,
    outputs: &'a mut [&'samples mut [Frame]],
    valid_frames: usize,
}

impl<'a, 'samples> OutputCapture<'a, 'samples> {
    pub fn new(plan: &'a OutputCapturePlan, outputs: &'a mut [&'samples mut [Frame]]) -> Self {
        Self {
            plan,
            outputs,
            valid_frames: 0,
        }
    }
    pub fn valid_frames(&self) -> usize {
        self.valid_frames
    }
    pub fn output(&self, index: usize) -> Option<&[Frame]> {
        self.outputs
            .get(index)
            .and_then(|frames| frames.get(..self.valid_frames))
    }
    pub(super) fn invalidate(&mut self) {
        self.valid_frames = 0;
        for output in self.outputs.iter_mut().take(MAX_OUTPUT_CAPTURES) {
            let end = output.len().min(MAX_OUTPUT_CAPTURE_FRAMES);
            output[..end].fill([0.0; 2]);
        }
    }
    /// Called only after validation; successful work scales with the block,
    /// while refusal retains the bounded full-slice cleanup above.
    pub(super) fn begin(&mut self, frames: usize) {
        self.valid_frames = 0;
        for output in self.outputs.iter_mut() {
            output[..frames].fill([0.0; 2]);
        }
    }
    pub(super) fn commit(&mut self, frames: usize) {
        self.valid_frames = frames;
    }
    pub(super) fn validate(
        &self,
        runtime: &SessionPlayback,
        frames: usize,
    ) -> Result<(), OutputCaptureError> {
        if !runtime.capture_cut_supported() {
            return Err(OutputCaptureError::UnsupportedRuntime);
        }
        if self.plan.runtime != runtime.capture_runtime
            || self.plan.epoch != runtime.capture_epoch
            || !Arc::ptr_eq(&self.plan.revision, &runtime.revision)
        {
            return Err(OutputCaptureError::StalePlan);
        }
        if frames > self.plan.frame_capacity
            || self.outputs.len() != self.plan.ids.len()
            || self
                .outputs
                .iter()
                .any(|output| output.len() < frames || output.len() > MAX_OUTPUT_CAPTURE_FRAMES)
        {
            return Err(OutputCaptureError::InsufficientCapacity);
        }
        if self
            .plan
            .ids
            .iter()
            .any(|id| !runtime.has_capture_owner(*id))
        {
            return Err(OutputCaptureError::MissingOwner);
        }
        Ok(())
    }
    pub(super) fn selects(&self, id: OutputCaptureId) -> bool {
        self.plan.ids.contains(&id)
    }
    pub(super) fn add(
        &mut self,
        id: OutputCaptureId,
        offset: usize,
        frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        let Some(index) = self.plan.ids.iter().position(|selected| *selected == id) else {
            return Ok(());
        };
        let end = offset
            .checked_add(frames.len())
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let destination = self
            .outputs
            .get_mut(index)
            .and_then(|output| output.get_mut(offset..end))
            .ok_or(SessionPlaybackError::Capture(
                OutputCaptureError::InsufficientCapacity,
            ))?;
        for (destination, source) in destination.iter_mut().zip(frames) {
            destination[0] += source[0];
            destination[1] += source[1];
        }
        Ok(())
    }
}

impl SessionPlayback {
    /// No callback plan building. Unknown and duplicate keys fail before allocation.
    pub fn output_capture_plan(
        &self,
        ids: &[OutputCaptureId],
        frame_capacity: usize,
    ) -> Result<OutputCapturePlan, OutputCaptureError> {
        if !self.capture_cut_supported() {
            return Err(OutputCaptureError::UnsupportedRuntime);
        }
        if frame_capacity == 0 || frame_capacity > MAX_OUTPUT_CAPTURE_FRAMES {
            return Err(OutputCaptureError::InsufficientCapacity);
        }
        if ids.len() > MAX_OUTPUT_CAPTURES {
            return Err(OutputCaptureError::TooManyOutputs);
        }
        for (index, id) in ids.iter().enumerate() {
            if ids[..index].contains(id) {
                return Err(OutputCaptureError::DuplicateOwner);
            }
            if !self.has_capture_owner(*id) {
                return Err(OutputCaptureError::MissingOwner);
            }
        }
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(ids.len())
            .map_err(|_| OutputCaptureError::Allocation)?;
        if selected.capacity() > MAX_OUTPUT_CAPTURES {
            return Err(OutputCaptureError::TooManyOutputs);
        }
        selected.extend_from_slice(ids);
        Ok(OutputCapturePlan {
            revision: Arc::clone(&self.revision),
            runtime: self.capture_runtime,
            epoch: self.capture_epoch,
            ids: selected,
            frame_capacity,
        })
    }
    fn has_capture_owner(&self, id: OutputCaptureId) -> bool {
        match id {
            OutputCaptureId::Track(id) => {
                self.routes.iter().any(|route| route.id == id)
                    || self.releases.iter().any(|release| {
                        release.location.tail_group().is_none()
                            && release
                                .source_origin
                                .map_or(release.track_id, |origin| origin.receiver)
                                == id
                    })
                    || self
                        .tail_groups
                        .iter()
                        .zip(&self.tail_track_ids)
                        .enumerate()
                        .any(|(index, (group, owner))| {
                            *owner == Some(id)
                                && group.as_ref().is_some_and(|chain| {
                                    !chain.is_empty()
                                        || self.releases.iter().any(|release| {
                                            release.location.tail_group().map(usize::from)
                                                == Some(index)
                                        })
                                        || self
                                            .tail_mutable_instruments
                                            .get(index)
                                            .and_then(Option::as_ref)
                                            .is_some_and(|tail| {
                                                !tail.runtime.committed_is_quiescent()
                                            })
                                })
                        })
            }
            OutputCaptureId::Return(id) => {
                self.revision
                    .arrangement()
                    .return_buses()
                    .iter()
                    .any(|bus| bus.id == id)
                    || self
                        .return_effect_tails
                        .slots()
                        .iter()
                        .flatten()
                        .any(|tail| tail.return_id() == Some(id))
            }
        }
    }

    // An explicit control-side cut in the existing canonical runtime. No
    // conversion, history reset or alternate scheduler is introduced.
    fn capture_cut_supported(&self) -> bool {
        (self.linear_arrangement_export
            || !self.loop_region().is_some_and(|region| region.enabled()))
            && self.routes.iter().all(|route| {
                route.owner == super::RouteOwner::Arrangement && route.pending.is_none()
            })
    }
}
