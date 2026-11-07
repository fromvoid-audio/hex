//! Control-owned, in-memory checkpoints for one mutable instrument source.
//!
//! This is a bounded state-copy foundation, not a seek implementation. A
//! checkpoint is valid only for the exact prepared revision, source receiver and
//! performance frame from which it was captured. It does not contain project
//! sources, route clocks, effect histories or a serialized runtime format.
use super::SessionPlayback;
use crate::{
    PreparedRevision,
    mutable_instrument::{
        MAX_MUTABLE_CHECKPOINT_STORE_BYTES, MAX_MUTABLE_CHECKPOINTS, MutableInstrumentRuntimeError,
        MutableInstrumentTrackRuntime,
    },
};
use hex_project::TrackId;
use std::{fmt, sync::Arc};

/// Maximum number of retained per-track checkpoints in one control-side store.
pub const MAX_MUTABLE_RUNTIME_CHECKPOINTS: usize = MAX_MUTABLE_CHECKPOINTS;
/// Aggregate upper bound for the store's inline slots and retained runtime
/// declarations/proof buffers. Restore has a separate 512 KiB operation peak
/// bound for this retained store plus its temporary candidate.
pub const MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES: usize = MAX_MUTABLE_CHECKPOINT_STORE_BYTES;

const MAX_MUTABLE_RUNTIME_CHECKPOINT_OPERATION_BYTES: usize =
    MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutableRuntimeCheckpointError {
    UnknownTrack,
    MissingMutableRuntime,
    UncommittedRuntime,
    IncompatiblePreparedRevision,
    IncompatibleSampleRate,
    IncompatiblePerformanceFrame,
    DuplicateCheckpoint,
    CheckpointUnavailable,
    CheckpointCountExceeded,
    CheckpointBudgetExceeded,
    RuntimeIncompatible,
    Allocation,
}

impl fmt::Display for MutableRuntimeCheckpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownTrack => "mutable runtime checkpoint track is unavailable",
            Self::MissingMutableRuntime => "track has no mutable instrument runtime",
            Self::UncommittedRuntime => "mutable runtime has an uncommitted block",
            Self::IncompatiblePreparedRevision => {
                "mutable runtime checkpoint belongs to another prepared revision"
            }
            Self::IncompatibleSampleRate => "mutable runtime checkpoint sample rate differs",
            Self::IncompatiblePerformanceFrame => {
                "mutable runtime checkpoint performance frame differs"
            }
            Self::DuplicateCheckpoint => "mutable runtime checkpoint already exists",
            Self::CheckpointUnavailable => "mutable runtime checkpoint is unavailable",
            Self::CheckpointCountExceeded => "mutable runtime checkpoint count limit exceeded",
            Self::CheckpointBudgetExceeded => "mutable runtime checkpoint byte limit exceeded",
            Self::RuntimeIncompatible => "mutable runtime checkpoint cannot be copied safely",
            Self::Allocation => "could not reserve mutable runtime checkpoint storage",
        })
    }
}

impl std::error::Error for MutableRuntimeCheckpointError {}

struct Entry {
    track: TrackId,
    performance_frame: u64,
    declared_storage_bytes: usize,
    runtime: MutableInstrumentTrackRuntime,
}

/// Bounded control-owned snapshots for one exact prepared graph identity.
///
/// The runtime value is `Send` but intentionally not `Sync`; do not share a
/// store with an active callback. Capture copies only a committed runtime and
/// its owned occurrence proofs/next ID. Restore is atomic with respect to its
/// target: all identity, capacity and compatibility checks complete on a
/// temporary candidate before the target owner is replaced. The supported
/// replay distance is exactly zero: restore requires the captured performance
/// frame. Nonzero replay, transport/source loop state, effects and full graph
/// reconstruction are outside this API.
pub struct MutableRuntimeCheckpointStore {
    revision: Option<Arc<PreparedRevision>>,
    sample_rate: Option<u32>,
    // Inline slots avoid post-preflight Vec growth and make every retained
    // slot part of `size_of::<Self>()`, even before the first capture.
    entries: [Option<Entry>; MAX_MUTABLE_RUNTIME_CHECKPOINTS],
    declared_storage_bytes: usize,
}

impl fmt::Debug for MutableRuntimeCheckpointStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MutableRuntimeCheckpointStore")
            .field("checkpoint_count", &self.checkpoint_count())
            .field("declared_storage_bytes", &self.declared_storage_bytes)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Default for MutableRuntimeCheckpointStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MutableRuntimeCheckpointStore {
    pub fn new() -> Self {
        Self {
            revision: None,
            sample_rate: None,
            entries: std::array::from_fn(|_| None),
            declared_storage_bytes: std::mem::size_of::<Self>(),
        }
    }

    pub fn checkpoint_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    pub fn declared_storage_bytes(&self) -> usize {
        self.declared_storage_bytes
    }

    /// Retain a committed snapshot of one source receiver at the playback's
    /// current performance frame. Requests through any bound contributor
    /// resolve to that source's single receiver checkpoint. Count and declared
    /// aggregate bytes are checked before the factory constructs the snapshot.
    pub fn capture_track(
        &mut self,
        playback: &SessionPlayback,
        track: TrackId,
    ) -> Result<(), MutableRuntimeCheckpointError> {
        let revision = &playback.revision;
        if revision.requires_control_sampler_sources() {
            return Err(MutableRuntimeCheckpointError::IncompatiblePreparedRevision);
        }
        if let Some(stored_revision) = &self.revision
            && !Arc::ptr_eq(stored_revision, revision)
        {
            return Err(MutableRuntimeCheckpointError::IncompatiblePreparedRevision);
        }
        let sample_rate = revision.arrangement().sample_rate();
        if self.sample_rate.is_some_and(|stored| stored != sample_rate) {
            return Err(MutableRuntimeCheckpointError::IncompatibleSampleRate);
        }
        let Some(slot_index) = self.entries.iter().position(Option::is_none) else {
            return Err(MutableRuntimeCheckpointError::CheckpointCountExceeded);
        };
        let requested_index = playback
            .routes
            .iter()
            .position(|route| route.id == track)
            .ok_or(MutableRuntimeCheckpointError::UnknownTrack)?;
        let route_index = playback
            .revision
            .source_routing()
            .receiver_index(requested_index)
            .unwrap_or(requested_index);
        let receiver = playback
            .routes
            .get(route_index)
            .map(|route| route.id)
            .ok_or(MutableRuntimeCheckpointError::UnknownTrack)?;
        if self.entries.iter().any(|entry| {
            entry.as_ref().is_some_and(|entry| {
                entry.track == receiver && entry.performance_frame == playback.performance_frame
            })
        }) {
            return Err(MutableRuntimeCheckpointError::DuplicateCheckpoint);
        }
        let source = playback
            .mutable_instruments
            .get(route_index)
            .and_then(Option::as_ref)
            .ok_or(MutableRuntimeCheckpointError::MissingMutableRuntime)?;
        if !source.committed_state_available() {
            return Err(MutableRuntimeCheckpointError::UncommittedRuntime);
        }
        let factory = revision
            .session()
            .mutable_instrument_factory(receiver)
            .ok_or(MutableRuntimeCheckpointError::MissingMutableRuntime)?;

        let charge =
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(factory, sample_rate)
                .and_then(|bytes| {
                    bytes.checked_add(if source.is_source_owner() {
                        MutableInstrumentTrackRuntime::source_live_storage_upper_bound()
                    } else {
                        0
                    })
                })
                .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;
        let next_payload_bytes = self
            .payload_bytes()
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?
            .checked_add(charge)
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;
        store_bytes(next_payload_bytes)
            .filter(|total| *total <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES)
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;
        let next_total = store_bytes(next_payload_bytes)
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;

        let mut snapshot =
            MutableInstrumentTrackRuntime::new(factory, sample_rate).map_err(map_runtime_error)?;
        if source.is_source_owner() {
            snapshot.enable_source_live();
        }
        snapshot.inherit_from(source).map_err(map_runtime_error)?;

        self.entries[slot_index] = Some(Entry {
            track: receiver,
            performance_frame: playback.performance_frame,
            declared_storage_bytes: charge,
            runtime: snapshot,
        });
        self.declared_storage_bytes = next_total;
        if self.revision.is_none() {
            self.revision = Some(Arc::clone(revision));
            self.sample_rate = Some(sample_rate);
        }
        Ok(())
    }

    /// Restore one source receiver at the exact captured frame, leaving the
    /// checkpoint reusable. The candidate is built and copied off to the side;
    /// the target owner changes only after validation and history copying
    /// succeed. Runtime retirement occurs on the exclusive caller thread,
    /// never inside the audio callback.
    pub fn restore_track(
        &self,
        playback: &mut SessionPlayback,
        track: TrackId,
    ) -> Result<(), MutableRuntimeCheckpointError> {
        let stored_revision = self
            .revision
            .as_ref()
            .ok_or(MutableRuntimeCheckpointError::CheckpointUnavailable)?;
        if !Arc::ptr_eq(stored_revision, &playback.revision) {
            return Err(MutableRuntimeCheckpointError::IncompatiblePreparedRevision);
        }
        let sample_rate = playback.revision.arrangement().sample_rate();
        if self.sample_rate != Some(sample_rate) {
            return Err(MutableRuntimeCheckpointError::IncompatibleSampleRate);
        }
        let requested_index = playback
            .routes
            .iter()
            .position(|route| route.id == track)
            .ok_or(MutableRuntimeCheckpointError::UnknownTrack)?;
        let route_index = playback
            .revision
            .source_routing()
            .receiver_index(requested_index)
            .unwrap_or(requested_index);
        let receiver = playback
            .routes
            .get(route_index)
            .map(|route| route.id)
            .ok_or(MutableRuntimeCheckpointError::UnknownTrack)?;
        let Some(entry) = self.entries.iter().flatten().find(|entry| {
            entry.track == receiver && entry.performance_frame == playback.performance_frame
        }) else {
            return if self
                .entries
                .iter()
                .flatten()
                .any(|entry| entry.track == receiver)
            {
                Err(MutableRuntimeCheckpointError::IncompatiblePerformanceFrame)
            } else {
                Err(MutableRuntimeCheckpointError::CheckpointUnavailable)
            };
        };
        let target = playback
            .mutable_instruments
            .get(route_index)
            .and_then(Option::as_ref)
            .ok_or(MutableRuntimeCheckpointError::MissingMutableRuntime)?;
        if !target.committed_state_available() {
            return Err(MutableRuntimeCheckpointError::UncommittedRuntime);
        }
        if target.is_source_owner() != entry.runtime.is_source_owner() {
            return Err(MutableRuntimeCheckpointError::RuntimeIncompatible);
        }
        let factory = playback
            .revision
            .session()
            .mutable_instrument_factory(receiver)
            .ok_or(MutableRuntimeCheckpointError::MissingMutableRuntime)?;
        let candidate_charge =
            MutableInstrumentTrackRuntime::checkpoint_storage_upper_bound(factory, sample_rate)
                .and_then(|bytes| {
                    bytes.checked_add(if entry.runtime.is_source_owner() {
                        MutableInstrumentTrackRuntime::source_live_storage_upper_bound()
                    } else {
                        0
                    })
                })
                .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;
        let candidate_charge = candidate_charge
            .checked_add(std::mem::size_of::<MutableInstrumentTrackRuntime>())
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;
        self.declared_storage_bytes
            .checked_add(candidate_charge)
            .filter(|total| *total <= MAX_MUTABLE_RUNTIME_CHECKPOINT_OPERATION_BYTES)
            .ok_or(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)?;

        let mut candidate =
            MutableInstrumentTrackRuntime::new(factory, sample_rate).map_err(map_runtime_error)?;
        if entry.runtime.is_source_owner() {
            candidate.enable_source_live();
        }
        candidate
            .inherit_from(&entry.runtime)
            .map_err(map_runtime_error)?;
        let Some(target) = playback
            .mutable_instruments
            .get_mut(route_index)
            .and_then(Option::as_mut)
        else {
            return Err(MutableRuntimeCheckpointError::MissingMutableRuntime);
        };
        *target = candidate;
        Ok(())
    }

    /// Drop all snapshots and release their runtimes on the calling control
    /// thread. Fixed inline slot storage remains charged. A later capture may
    /// bind the store to another exact revision.
    pub fn clear(&mut self) {
        self.entries = std::array::from_fn(|_| None);
        self.declared_storage_bytes = std::mem::size_of::<Self>();
        self.revision = None;
        self.sample_rate = None;
    }

    fn payload_bytes(&self) -> Option<usize> {
        self.entries
            .iter()
            .flatten()
            .try_fold(0usize, |total, entry| {
                total.checked_add(entry.declared_storage_bytes)
            })
    }
}

fn store_bytes(payload_bytes: usize) -> Option<usize> {
    std::mem::size_of::<MutableRuntimeCheckpointStore>().checked_add(payload_bytes)
}

fn map_runtime_error(error: MutableInstrumentRuntimeError) -> MutableRuntimeCheckpointError {
    match error {
        MutableInstrumentRuntimeError::Allocation => MutableRuntimeCheckpointError::Allocation,
        MutableInstrumentRuntimeError::Budget => {
            MutableRuntimeCheckpointError::CheckpointBudgetExceeded
        }
        _ => MutableRuntimeCheckpointError::RuntimeIncompatible,
    }
}
