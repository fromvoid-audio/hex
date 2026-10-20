//! Typed bounded export records. This is control custody/bookkeeping only;
//! persistent service registration and full joint admission remain mandatory.
use crate::service::ViewState;
use hex_engine::SharedMetadata;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

static NEXT_FAMILY: AtomicU64 = AtomicU64::new(1);

mod event;
mod native_editor_profile;
mod snapshot_retirement;
mod source_event;
mod source_queue;
pub use native_editor_profile::NATIVE_PLAYBACK_CANDIDATE_CAPACITY;
pub use source_event::{
    SourceBatchFailureRead, SourceEventFacts, SourceEventMetadata, SourceEventRead,
    SourceEventSourceRelation, SourceGestureAckRead, SourceTuningGestureAckRead,
    SourceTuningLibraryRead, SourceTuningSlotRead,
};
pub use source_queue::{SourceEventDelivery, SourceEventHead};
mod family;
mod model_read;
pub use model_read::ui_consumers;
pub use model_read::{
    SnapshotClipRead, SnapshotDeviceRead, SnapshotProjectRead, SnapshotReturnRead,
    SnapshotTrackRead,
};
// Record transitions are independent of the platform exclusion primitive. The
// existing family supplies structural exclusion; Service wiring is in progress.
mod native_export_results;
mod native_requests;
pub use native_export_results::{ExportReportRead, ExportResultRead};
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native Service transport integration in progress")
)]
mod native_transport;
pub use native_requests::{NativeRequestDelivery, NativeRequestStamp};
pub use native_transport::CompiledStamp as NativeCompiledStamp;
mod storage;
mod transport;
mod worker_control;
mod worker_export_results;
mod worker_requests;
pub use family::{
    SessionExportFamily, SnapshotEventConstruction, SnapshotFamilyCleanup,
    SnapshotFamilyInventoryGuard, SnapshotFamilyQueries, SnapshotProducerKey,
};
pub(crate) use transport::SnapshotSourceEventSender;
pub use transport::{
    SnapshotEventReads, SnapshotReadCapability, SnapshotReadContinuation, SnapshotReadOperations,
    SnapshotSourceEventOperations, SnapshotSourceEventReceiver, SnapshotSourceKey,
    SnapshotTransportError, SourceEventHeadContinuation, SourceEventHeadRelationContinuation,
    SourceEventReadDelivery,
};
pub use worker_control::{SnapshotWorkerControl, SnapshotWorkerOperations, SnapshotWorkerStop};
pub(crate) use worker_export_results::SnapshotExportResultEndpoint;
pub use worker_export_results::{
    ExportResultContinuation, NativeExportResultSubmission, SnapshotExportResultOperations,
};
pub(crate) use worker_requests::SnapshotRequestEndpoint;
pub use worker_requests::{NativeRequestSubmission, SnapshotRequestOperations};

/// Component limits, taken from the same document/control reservation. They
/// neither grant a new DATA allowance nor quote snapshot children/whole peak.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotExportReservation {
    pub snapshot_records: usize,
    pub read_records: usize,
    pub max_bookkeeping_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRecordError {
    Reservation,
    Allocation,
    Exhausted,
    Stale,
    Closed,
    Busy,
}
/// Nonowning producer correlation only. A stamp is not a source lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotStamp {
    family: u64,
    index: usize,
    generation: u64,
}
/// One reserved read; deliberately not Clone/Copy. Derivation reserves another
/// actual read slot. Its eventual public capability must release through the
/// already-registered service, never by returning final snapshot ownership.
#[derive(Debug)]
pub struct SnapshotReadStamp {
    family: u64,
    index: usize,
    generation: u64,
    snapshot: SnapshotStamp,
}
struct SnapshotRecord {
    generation: u64,
    snapshot: Option<SharedMetadata<ViewState>>,
    event_metadata: Option<SharedMetadata<source_event::SourceEventMetadata>>,
    retired: bool,
    readers: AtomicUsize,
    cleanup_started: bool,
}
struct ReadRecord {
    generation: AtomicU64,
    snapshot_index: AtomicUsize,
    snapshot_generation: AtomicU64,
    occupied: AtomicBool,
    released: AtomicBool,
}
impl ReadRecord {
    fn vacant() -> Self {
        Self {
            generation: AtomicU64::new(0),
            snapshot_index: AtomicUsize::new(0),
            snapshot_generation: AtomicU64::new(0),
            occupied: AtomicBool::new(false),
            released: AtomicBool::new(false),
        }
    }
}
/// Actual typed records owned by control. No Arc/Weak, ViewState, SharedProject,
/// waveform owner or resource lease is exported by this API. This value must
/// enter persistent service custody before any future UI capability escapes.
pub struct SnapshotExportRecords {
    family: u64,
    revision: u64,
    closed: bool,
    snapshots: Vec<SnapshotRecord>,
    reads: Vec<ReadRecord>,
    native_compiled: Option<native_transport::NativeCompiledRecords>,
    native_requests: native_requests::NativeRequests,
    native_export_result: native_export_results::NativeExportResult,
    source_events: source_queue::SourceEventQueue,
    max_bookkeeping_bytes: usize,
}
/// A scoped read surface; even the underlying ViewState cannot be extracted.
/// Copies are only borrowed queries, not registered readers or owners.
#[derive(Clone, Copy)]
pub struct SnapshotRead<'a> {
    snapshot: &'a ViewState,
    event: Option<&'a source_event::SourceEventMetadata>,
}
impl<'a> SnapshotRead<'a> {
    pub fn revision(self) -> u64 {
        self.snapshot.revision
    }
    pub fn sample_rate(self) -> u32 {
        self.snapshot.project.sample_rate()
    }
    pub fn waveform(self, asset: hex_project::AssetId) -> Option<crate::WaveformRead<'a>> {
        self.snapshot
            .waveforms
            .get(&asset)
            .map(crate::read_shared_waveform)
    }
    /// Resolve the selected Sampler's authored asset to this EXACT snapshot's
    /// existing cached waveform. Display correlation only, never resource/native
    /// admission or PCM identity proof. No waveform owner escapes the read scope.
    /// Empty/missing cached summary is explicit None; missing authored media or
    /// out-of-source bounds is an error, never invented peaks.
    /// ```compile_fail
    /// fn escape(records: &hex_session::snapshot_exports::SnapshotExportRecords,
    ///     stamp: &hex_session::snapshot_exports::SnapshotReadStamp,
    ///     device: hex_project::DeviceId) {
    ///     let _escaped = records.read_with(stamp, |snapshot| snapshot.sampler_waveform(device));
    /// }
    /// ```
    pub fn sampler_waveform(
        self,
        device: hex_project::DeviceId,
    ) -> Result<Option<crate::WaveformRead<'a>>, hex_lattice_contract::DeviceContractError> {
        use hex_lattice_contract::DeviceContractError as Error;
        let authored = match self.sampler_state(device)? {
            hex_engine::SamplerDeviceStateRead::Empty => return Ok(None),
            hex_engine::SamplerDeviceStateRead::Sample(authored) => authored,
        };
        let state = self
            .snapshot
            .project
            .device(device)
            .ok_or(Error::WrongDeviceType)?
            .state();
        let reference = state
            .asset_references()
            .first()
            .ok_or(Error::AssetResourcesUnavailable)?;
        let asset = self
            .snapshot
            .project
            .assets()
            .find(|asset| asset.id().value() == reference.asset.value())
            .ok_or(Error::AssetResourcesUnavailable)?;
        let Some(waveform) = self.waveform(asset.id()) else {
            return Ok(None);
        };
        if waveform.source_frames() as u64 != asset.source_frames()
            || authored.region().end > waveform.source_frames()
        {
            return Err(Error::AssetResourcesUnavailable);
        }
        Ok(Some(waveform))
    }
    /// Validated authored scalar/marker queries on this exact retained model.
    /// No NativeDeviceState/Project/owning child can be extracted. Empty state
    /// stays empty; missing devices and unsupported state are explicit errors.
    /// ```compile_fail
    /// fn escape(records: &hex_session::snapshot_exports::SnapshotExportRecords,
    ///     stamp: &hex_session::snapshot_exports::SnapshotReadStamp,
    ///     device: hex_project::DeviceId) {
    ///     let _escaped = records.read_with(stamp, |snapshot| snapshot.sampler_state(device));
    /// }
    /// ```
    pub fn sampler_state(
        self,
        device: hex_project::DeviceId,
    ) -> Result<hex_engine::SamplerDeviceStateRead<'a>, hex_lattice_contract::DeviceContractError>
    {
        let state = self
            .snapshot
            .project
            .device(device)
            .ok_or(hex_lattice_contract::DeviceContractError::WrongDeviceType)?
            .state();
        hex_engine::SamplerDeviceStateRead::from_native(state)
    }

    /// Fixed Copy control facts from this EXACT retained Sampler. Finish the
    /// read before GPUI/render/submit/ACK; no state, model or resource escapes.
    /// This metadata query does not open the instrument registry or processing.
    pub fn sampler_parameters(
        self,
        device: hex_project::DeviceId,
    ) -> Result<
        [hex_engine::NativeDeviceParameterView; 43],
        hex_lattice_contract::DeviceContractError,
    > {
        let state = self
            .snapshot
            .project
            .device(device)
            .ok_or(hex_lattice_contract::DeviceContractError::WrongDeviceType)?
            .state();
        hex_engine::sampler_parameters_on_control(state)
    }
}
impl SnapshotExportRecords {
    /// Requested record-table component quote before allocating either backing.
    /// Snapshot bodies, children, enclosing shared header and preparation peak
    /// remain separately charged by the actual registered-family constructor.
    pub fn reservation_quote(
        limits: SnapshotExportReservation,
    ) -> Result<usize, ExportRecordError> {
        if limits.snapshot_records == 0 || limits.read_records == 0 {
            return Err(ExportRecordError::Reservation);
        }
        std::mem::size_of::<Self>()
            .checked_add(
                limits
                    .snapshot_records
                    .checked_mul(std::mem::size_of::<SnapshotRecord>())
                    .ok_or(ExportRecordError::Reservation)?,
            )
            .and_then(|n| {
                n.checked_add(
                    limits
                        .read_records
                        .checked_mul(std::mem::size_of::<ReadRecord>())?,
                )
            })
            .filter(|bytes| *bytes <= limits.max_bookkeeping_bytes)
            .ok_or(ExportRecordError::Reservation)
    }
    pub fn try_new_on_control(
        limits: SnapshotExportReservation,
    ) -> Result<Self, ExportRecordError> {
        Self::reservation_quote(limits)?;
        let mut snapshots = Vec::new();
        let mut reads = Vec::new();
        snapshots
            .try_reserve_exact(limits.snapshot_records)
            .map_err(|_| ExportRecordError::Allocation)?;
        reads
            .try_reserve_exact(limits.read_records)
            .map_err(|_| ExportRecordError::Allocation)?;
        // The allocator need not honor requested capacity exactly. Observe both
        // actual backings before constructing records; never assume equality.
        let actual = std::mem::size_of::<Self>()
            .checked_add(
                snapshots
                    .capacity()
                    .checked_mul(std::mem::size_of::<SnapshotRecord>())
                    .ok_or(ExportRecordError::Reservation)?,
            )
            .and_then(|n| {
                n.checked_add(
                    reads
                        .capacity()
                        .checked_mul(std::mem::size_of::<ReadRecord>())?,
                )
            })
            .ok_or(ExportRecordError::Reservation)?;
        if actual > limits.max_bookkeeping_bytes {
            return Err(ExportRecordError::Reservation);
        }
        snapshots.resize_with(limits.snapshot_records, || SnapshotRecord {
            generation: 0,
            snapshot: None,
            event_metadata: None,
            retired: false,
            readers: AtomicUsize::new(0),
            cleanup_started: false,
        });
        reads.resize_with(limits.read_records, ReadRecord::vacant);
        let family = NEXT_FAMILY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| ExportRecordError::Exhausted)?;
        Ok(Self {
            family,
            revision: 1,
            closed: false,
            snapshots,
            reads,
            native_compiled: None,
            native_requests: Default::default(),
            native_export_result: Default::default(),
            source_events: Default::default(),
            max_bookkeeping_bytes: limits.max_bookkeeping_bytes,
        })
    }
    pub fn inventory_revision(&self) -> u64 {
        self.revision
    }
    /// The actual pending source slot is unchanged on refusal. No boxing or
    /// hidden error allocation is needed to return a still-unexported owner.
    pub fn insert_on_control(
        &mut self,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotStamp, ExportRecordError> {
        if self.closed {
            return Err(ExportRecordError::Closed);
        }
        if source.is_none() {
            return Err(ExportRecordError::Stale);
        }
        let index = self
            .snapshots
            .iter()
            .position(|record| {
                record.snapshot.is_none()
                    && record.event_metadata.is_none()
                    && !record.cleanup_started
            })
            .ok_or(ExportRecordError::Reservation)?;
        let generation = self.snapshots[index]
            .generation
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let snapshot = source.take().ok_or(ExportRecordError::Stale)?;
        let record = &mut self.snapshots[index];
        record.snapshot = Some(SharedMetadata::new_on_control(snapshot));
        record.generation = generation;
        record.retired = false;
        record.cleanup_started = false;
        self.revision = revision;
        Ok(SnapshotStamp {
            family: self.family,
            index,
            generation,
        })
    }
    fn record(&self, stamp: SnapshotStamp) -> Result<&SnapshotRecord, ExportRecordError> {
        self.snapshots
            .get(stamp.index)
            .filter(|record| {
                stamp.family == self.family
                    && record.generation == stamp.generation
                    && record.snapshot.is_some()
            })
            .ok_or(ExportRecordError::Stale)
    }
    /// Read bookkeeping is preallocated and introduces no owning child or
    /// allocation. The surrounding family guard protects record identity.
    pub fn reserve_read(
        &self,
        snapshot: SnapshotStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        let record = self.record(snapshot)?;
        if self.closed || record.retired {
            return Err(ExportRecordError::Closed);
        }
        self.reserve_read_record(snapshot, record)
    }
    fn reserve_read_record(
        &self,
        snapshot: SnapshotStamp,
        record: &SnapshotRecord,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        for (index, read) in self.reads.iter().enumerate() {
            if read
                .occupied
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            let generation =
                match read
                    .generation
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
                {
                    Ok(previous) => previous + 1,
                    Err(_) => {
                        read.occupied.store(false, Ordering::Release);
                        continue;
                    }
                };
            if record
                .readers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
                .is_err()
            {
                read.occupied.store(false, Ordering::Release);
                return Err(ExportRecordError::Exhausted);
            }
            read.snapshot_index.store(snapshot.index, Ordering::Relaxed);
            read.snapshot_generation
                .store(snapshot.generation, Ordering::Relaxed);
            read.released.store(false, Ordering::Release);
            return Ok(SnapshotReadStamp {
                family: self.family,
                index,
                generation,
                snapshot,
            });
        }
        Err(ExportRecordError::Reservation)
    }
    fn read_record(&self, stamp: &SnapshotReadStamp) -> Result<&ReadRecord, ExportRecordError> {
        self.reads
            .get(stamp.index)
            .filter(|read| {
                stamp.family == self.family
                    && read.generation.load(Ordering::Acquire) == stamp.generation
                    && read.occupied.load(Ordering::Acquire)
                    && read.snapshot_index.load(Ordering::Acquire) == stamp.snapshot.index
                    && read.snapshot_generation.load(Ordering::Acquire) == stamp.snapshot.generation
            })
            .ok_or(ExportRecordError::Stale)
    }
    pub fn derive_read(
        &self,
        existing: &SnapshotReadStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        if self.read_record(existing)?.released.load(Ordering::Acquire) {
            return Err(ExportRecordError::Stale);
        }
        let record = self.record(existing.snapshot)?;
        self.reserve_read_record(existing.snapshot, record)
    }
    /// Reads cannot escape their exact protected snapshot.
    /// ```compile_fail
    /// use hex_session::snapshot_exports::{SnapshotExportRecords, SnapshotReadStamp};
    /// fn escape(records: &SnapshotExportRecords, stamp: &SnapshotReadStamp) {
    ///     let _escaped = records.read_with(stamp, |read| read);
    /// }
    /// ```
    pub fn read_with<R>(
        &self,
        stamp: &SnapshotReadStamp,
        read: impl for<'view> FnOnce(SnapshotRead<'view>) -> R,
    ) -> Result<R, ExportRecordError> {
        Ok(read(SnapshotRead {
            snapshot: self.snapshot_for_read(stamp)?,
            event: self.record(stamp.snapshot)?.event_metadata.as_deref(),
        }))
    }
    fn snapshot_for_read(
        &self,
        stamp: &SnapshotReadStamp,
    ) -> Result<&ViewState, ExportRecordError> {
        if self.read_record(stamp)?.released.load(Ordering::Acquire) {
            return Err(ExportRecordError::Stale);
        }
        self.record(stamp.snapshot)?
            .snapshot
            .as_deref()
            .ok_or(ExportRecordError::Stale)
    }
    /// Ordered authoring correlation of TWO actual live reads. Both records
    /// stay borrowed under the SAME family exclusion; no nested acquisition.
    /// This boolean conveys neither native admission nor a storage receipt.
    pub fn follows_source(
        &self,
        current: &SnapshotReadStamp,
        previous: &SnapshotReadStamp,
        generation: u64,
    ) -> Result<bool, ExportRecordError> {
        let current = self.snapshot_for_read(current)?;
        let previous = self.snapshot_for_read(previous)?;
        Ok(current
            .comparison_audio
            .as_ref()
            .is_some_and(|binding| binding.follows(previous, current, generation)))
    }
    /// Durable release only: no snapshot destruction, byte-release claim or
    /// owning enqueue. The service later processes this exact incarnation.
    pub fn release_read(&self, stamp: SnapshotReadStamp) -> Result<(), ExportRecordError> {
        let read = self.read_record(&stamp)?;
        read.released.store(true, Ordering::Release);
        Ok(())
    }
    pub fn retire_on_control(&mut self, stamp: SnapshotStamp) -> Result<(), ExportRecordError> {
        self.record(stamp)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        self.snapshots[stamp.index].retired = true;
        self.revision = revision;
        Ok(())
    }
    /// Revoke new producer reads while retaining all existing read incarnations.
    /// Native endpoint retirement must not wait for these application records.
    pub fn close_on_control(&mut self) -> Result<(), ExportRecordError> {
        if self.closed {
            return Ok(());
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        for record in &mut self.snapshots {
            record.retired = true;
        }
        self.closed = true;
        self.revision = revision;
        Ok(())
    }
    /// Drain release bookkeeping only. Owning cleanup and persistent destructor
    /// failure phases belong to the already-reserved lifecycle adapter.
    pub fn collect_releases_on_control(&mut self) -> Result<usize, ExportRecordError> {
        let mut count = 0;
        for read in &self.reads {
            if !read.occupied.load(Ordering::Acquire) || !read.released.load(Ordering::Acquire) {
                continue;
            }
            let snapshot = self
                .snapshots
                .get(read.snapshot_index.load(Ordering::Acquire))
                .filter(|record| {
                    record.generation == read.snapshot_generation.load(Ordering::Acquire)
                        && record.snapshot.is_some()
                })
                .ok_or(ExportRecordError::Stale)?;
            snapshot
                .readers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                .map_err(|_| ExportRecordError::Stale)?;
            read.occupied.store(false, Ordering::Release);
            count += 1;
        }
        Ok(count)
    }
    /// Actual backings only; enclosing storage and each snapshot body/children
    /// require their typed inventory. No complete/known flag is produced.
    pub fn visit_backings_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        ledger.borrow_metadata_vec_backing_on_control(&self.snapshots)?;
        ledger.borrow_metadata_vec_backing_on_control(&self.reads)
    }
}

#[cfg(test)]
mod tests;
