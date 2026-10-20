//! Concrete Session adapter payload for the existing lifecycle family hook.
//! No audio/GPUI edge, owning child export, registry admission or byte release.
use super::*;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

/// One actual existing family writer, acquired before preparing an Event view.
/// This owns no snapshot or approval. Its private destination cannot escape;
/// normal release without insertion changes no table, generation or reader.
/// Complete before-birth DATA/peak inspection and all other guards remain the
/// closed caller's obligations. End every immutable projection before insert,
/// and release this writer before read binding, ACK, enqueue, DSP or file I/O.
/// ```compile_fail
/// fn escape(family: &hex_session::snapshot_exports::SessionExportFamily,
///     key: &hex_session::snapshot_exports::SnapshotProducerKey)
///     -> hex_session::snapshot_exports::SnapshotEventConstruction<'static> {
///     family.try_event_construction_on_control(key).unwrap()
/// }
/// ```
/// ```compile_fail
/// fn send(loan: hex_session::snapshot_exports::SnapshotEventConstruction<'static>) {
///     std::thread::spawn(move || drop(loan));
/// }
/// ```
/// ```compile_fail
/// fn overlap(loan: hex_session::snapshot_exports::SnapshotEventConstruction<'_>,
///     source: &mut Option<hex_session::service::ViewState>) {
///     let observed = loan.sampler_family_fields_on_control().unwrap();
///     let _ = loan.insert_on_control(source);
///     drop(observed);
/// }
/// ```
pub struct SnapshotEventConstruction<'a> {
    owner: &'a SessionExportFamily,
    records: RwLockWriteGuard<'a, SnapshotExportRecords>,
    destination: super::event::EventDestination,
}
impl SnapshotEventConstruction<'_> {
    /// Mandatory actual family/retiring storage under this SAME writer. The
    /// projection cannot survive the loan or overlap its consuming insertion.
    /// ```compile_fail
    /// fn escape<'a>(loan: hex_session::snapshot_exports::SnapshotEventConstruction<'a>)
    ///     -> hex_engine::sampler_host_fields::SamplerFamilyGuardFields<'a> {
    ///     loan.sampler_family_fields_on_control().unwrap()
    /// }
    /// ```
    pub fn sampler_family_fields_on_control(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::SamplerFamilyGuardFields<'_>,
        hex_engine::AudioStorageError,
    > {
        self.owner.retiring_request.visit()?;
        self.owner.retiring_snapshot.require_vacant_storage()?;
        Ok(hex_engine::sampler_host_fields::SamplerFamilyGuardFields {
            guard: hex_engine::sampler_host_fields::BodyRef::of(self),
            owner: hex_engine::sampler_host_fields::BodyRef::of(self.owner),
            records: &*self.records,
            retiring_compiled: self.owner.retiring_compiled.sampler_fields()?,
            retiring_export: self.owner.retiring_export_result.sampler_fields()?,
        })
    }
    /// No body is extracted. Refusal preserves the actual pending view; a
    /// successful fresh paired snapshot and initial read become visible when
    /// this actual writer releases. This operation supplies no native seal.
    /// Attach worker-only immutable metadata to the SAME view/read record.
    /// Both fresh paired constructors and all children require the closed
    /// caller's before-birth normal/bootstrap and retained/peak preflight.
    /// This is not a native admission/publication or legacy adoption API.
    pub fn insert_source_on_control(
        self,
        source: &mut Option<ViewState>,
        metadata: &mut Option<super::SourceEventMetadata>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        let Self {
            owner,
            mut records,
            destination,
        } = self;
        if owner.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.insert_source_event_at_destination_on_control(destination, source, metadata)
    }
    pub fn insert_on_control(
        self,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        let Self {
            owner,
            mut records,
            destination,
        } = self;
        if owner.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.insert_event_at_destination_on_control(destination, source)
    }
}

/// Exactly one active producer receives this correlation at construction.
/// It carries no family/snapshot owner and cannot fabricate a read capability.
#[derive(Debug)]
pub struct SnapshotProducerKey {
    family: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFamilyCleanup {
    Pending,
    Disposed,
}
/// Stored once in persistent lifecycle custody before any frame escape.
/// Its private record lock also guards actual inventory through publication.
/// Neither this payload nor its producer key is a native endpoint/device lease.
pub struct SessionExportFamily {
    identity: u64,
    records: RwLock<SnapshotExportRecords>,
    abandoned: AtomicBool,
    retiring_compiled: native_transport::CompiledRetirement,
    retiring_request: native_requests::RequestRetirement,
    retiring_export_result: native_export_results::ExportResultRetirement,
    retiring_snapshot: super::snapshot_retirement::SnapshotRetirement,
}
/// Control-only structural inventory guard. No records, model, waveform owner
/// or mutable payload escapes. Atomics for ACK/derive/abandon are NOT frozen.
/// Kept on its acquiring thread; outer job guard must outlive this guard.
/// ```compile_fail
/// fn transfer(guard: hex_session::snapshot_exports::SnapshotFamilyInventoryGuard<'static>) {
///     std::thread::spawn(move || drop(guard));
/// }
/// ```
/// ```compile_fail
/// fn widen<'a>(guards: &'a Vec<hex_session::snapshot_exports::SnapshotFamilyInventoryGuard<'a>>)
///     -> hex_engine::AudioRootStorageLedger<'static> {
///     let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
///         hex_engine::AudioStorageLedgerReservation { data_rows: 8, handle_rows: 8,
///             root_rows: 4, max_retained_bytes: 100000, max_construction_peak_bytes: 100000 }).unwrap();
///     ledger.borrow_metadata_vec_backing_on_control(guards).unwrap();
///     ledger
/// }
/// ```
pub struct SnapshotFamilyInventoryGuard<'a> {
    owner: &'a SessionExportFamily,
    records: RwLockReadGuard<'a, SnapshotExportRecords>,
    retiring_compiled: &'a native_transport::CompiledRetirement,
    retiring_request: &'a native_requests::RequestRetirement,
    retiring_export_result: &'a native_export_results::ExportResultRetirement,
}
impl SnapshotFamilyInventoryGuard<'_> {
    /// The fields cannot outlive the ACTUAL inner exclusion or its parent.
    /// ```compile_fail
    /// fn escape<'a>(guard: hex_session::snapshot_exports::SnapshotFamilyInventoryGuard<'a>)
    ///     -> hex_engine::sampler_host_fields::SamplerFamilyGuardFields<'a> {
    ///     guard.sampler_family_fields_on_control().unwrap()
    /// }
    /// ```
    pub fn sampler_family_fields_on_control(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::SamplerFamilyGuardFields<'_>,
        hex_engine::AudioStorageError,
    > {
        let Self {
            owner,
            records,
            retiring_compiled,
            retiring_request,
            retiring_export_result,
        } = self;
        retiring_request.visit()?;
        Ok(hex_engine::sampler_host_fields::SamplerFamilyGuardFields {
            guard: hex_engine::sampler_host_fields::BodyRef::of(self),
            owner: hex_engine::sampler_host_fields::BodyRef::of(*owner),
            records: &**records,
            retiring_compiled: retiring_compiled.sampler_fields()?,
            retiring_export: retiring_export_result.sampler_fields()?,
        })
    }
    pub fn inventory_revision(&self) -> u64 {
        self.records.inventory_revision()
    }
    pub fn snapshot_count(&self) -> usize {
        self.records
            .snapshots
            .iter()
            .filter(|record| record.snapshot.is_some())
            .count()
    }
    pub fn visit_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.records.visit_selected_storage_on_control(ledger)?;
        self.retiring_compiled.visit(ledger)?;
        self.retiring_request.visit()?;
        self.retiring_export_result.visit(ledger)
    }
}
impl SessionExportFamily {
    /// Single-try structural exclusion and destination check, with no reserve,
    /// record mutation, metadata allocation or payload construction. Hold the
    /// existing outer family custody first. This is not a transferable permit.
    pub fn try_event_construction_on_control(
        &self,
        key: &SnapshotProducerKey,
    ) -> Result<SnapshotEventConstruction<'_>, ExportRecordError> {
        self.validate_producer_on_control(key)?;
        if !self.retiring_snapshot.vacant() {
            return Err(ExportRecordError::Stale);
        }
        let records = self.records.try_write().map_err(|error| match error {
            TryLockError::WouldBlock => ExportRecordError::Busy,
            TryLockError::Poisoned(_) => ExportRecordError::Stale,
        })?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let destination = records.event_destination_on_control()?;
        Ok(SnapshotEventConstruction {
            owner: self,
            records,
            destination,
        })
    }
    /// Correlation/liveness check under the actual outer holder guard; it does
    /// not freeze later cancellation, inventory or publication.
    pub fn validate_producer_on_control(
        &self,
        key: &SnapshotProducerKey,
    ) -> Result<(), ExportRecordError> {
        if key.family != self.identity {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        Ok(())
    }

    /// Acquire AFTER all actual outer slots and BEFORE ledger/native barrier.
    /// Poison refuses. Failure consumes no source/reader and changes no custody.
    /// ```compile_fail
    /// fn escape(family: &hex_session::snapshot_exports::SessionExportFamily)
    ///     -> hex_session::snapshot_exports::SnapshotFamilyInventoryGuard<'static> {
    ///     family.try_inventory_guard_on_control().unwrap()
    /// }
    /// ```
    pub fn try_inventory_guard_on_control(
        &self,
    ) -> Result<SnapshotFamilyInventoryGuard<'_>, ExportRecordError> {
        if !self.retiring_snapshot.vacant() {
            return Err(ExportRecordError::Stale);
        }
        let Self {
            identity,
            records,
            abandoned,
            retiring_compiled,
            retiring_request,
            retiring_export_result,
            retiring_snapshot,
        } = self;
        retiring_snapshot
            .require_vacant_storage()
            .map_err(|_| ExportRecordError::Stale)?;
        let _: (&u64, &AtomicBool) = (identity, abandoned);
        let records = records.try_read().map_err(|error| match error {
            TryLockError::WouldBlock => ExportRecordError::Busy,
            TryLockError::Poisoned(_) => ExportRecordError::Stale,
        })?;
        Ok(SnapshotFamilyInventoryGuard {
            owner: self,
            records,
            retiring_compiled,
            retiring_request,
            retiring_export_result,
        })
    }
    pub fn try_new_on_control(
        limits: SnapshotExportReservation,
    ) -> Result<(Self, SnapshotProducerKey), ExportRecordError> {
        let overhead = std::mem::size_of::<Self>()
            .checked_sub(std::mem::size_of::<SnapshotExportRecords>())
            .ok_or(ExportRecordError::Reservation)?;
        let mut record_limits = limits;
        record_limits.max_bookkeeping_bytes = limits
            .max_bookkeeping_bytes
            .checked_sub(overhead)
            .ok_or(ExportRecordError::Reservation)?;
        let records = SnapshotExportRecords::try_new_on_control(record_limits)?;
        let key = SnapshotProducerKey {
            family: records.family,
        };
        Ok((
            Self {
                identity: records.family,
                records: RwLock::new(records),
                abandoned: AtomicBool::new(false),
                retiring_compiled: native_transport::CompiledRetirement::default(),
                retiring_request: Default::default(),
                retiring_export_result: Default::default(),
                retiring_snapshot: Default::default(),
            },
            key,
        ))
    }
    /// Same registered family and SAME record lock. Inline optional storage is
    /// already part of the ordinary constructor's actual body quote; no backing,
    /// lock or registry is allocated when enabling these two bounded slots.
    pub fn try_new_native_on_control(
        limits: SnapshotExportReservation,
    ) -> Result<(Self, SnapshotProducerKey), ExportRecordError> {
        let (mut family, key) = Self::try_new_on_control(limits)?;
        family
            .records
            .get_mut()
            .map_err(|_| ExportRecordError::Stale)?
            .native_compiled = Some(native_transport::NativeCompiledRecords::new(
            family.identity,
        ));
        Ok((family, key))
    }
    fn with_native_records<R>(
        &self,
        key: &SnapshotProducerKey,
        operation: impl FnOnce(
            &mut native_transport::NativeCompiledRecords,
        ) -> Result<R, ExportRecordError>,
    ) -> Result<R, ExportRecordError> {
        self.validate_producer_on_control(key)?;
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        self.validate_producer_on_control(key)?;
        let revision = records
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let native = records
            .native_compiled
            .as_mut()
            .ok_or(ExportRecordError::Stale)?;
        let result = operation(native)?;
        records.revision = revision;
        Ok(result)
    }
    pub fn reserve_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
    ) -> Result<NativeCompiledStamp, ExportRecordError> {
        self.with_native_records(key, |records| records.reserve())
    }
    /// Caller retains its actual producer payload unchanged on refusal. This
    /// fixed transfer is not constructor/root/publication admission.
    pub fn insert_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
        stamp: NativeCompiledStamp,
        source: &mut Option<hex_engine::AudioCompiledRevision>,
    ) -> Result<(), ExportRecordError> {
        self.with_native_records(key, |records| records.install(stamp, source))
    }
    /// Duplicate straight into an already reserved field in this same family.
    /// A CAS collision returns Busy; both source and target remain rooted.
    pub fn duplicate_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
        source: NativeCompiledStamp,
        target: NativeCompiledStamp,
    ) -> Result<(), ExportRecordError> {
        self.with_native_records(key, |records| records.duplicate(source, target))
    }
    pub fn claim_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
        stamp: NativeCompiledStamp,
    ) -> Result<(), ExportRecordError> {
        self.with_native_records(key, |records| records.claim(stamp))
    }
    pub fn complete_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
        stamp: NativeCompiledStamp,
    ) -> Result<(), ExportRecordError> {
        self.with_native_records(key, |records| records.complete(stamp))
    }
    pub fn retire_compiled_on_control(
        &self,
        key: &SnapshotProducerKey,
        stamp: NativeCompiledStamp,
    ) -> Result<(), ExportRecordError> {
        self.with_native_records(key, |records| records.retire(stamp))
    }
    /// Scalar read only. No public callback can clone an unrooted owner.
    pub fn compiled_metadata_on_control(
        &self,
        stamp: NativeCompiledStamp,
    ) -> Result<hex_engine::AudioPreparedRevisionMetadata, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        Ok(records
            .native_compiled
            .as_ref()
            .ok_or(ExportRecordError::Stale)?
            .borrowed(stamp)?
            .metadata())
    }
    /// Reserve actual native request backing before a bound Service can escape.
    pub fn prepare_requests_on_control(&self) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let existing = std::mem::size_of::<SnapshotExportRecords>()
            .checked_add(
                records
                    .snapshots
                    .capacity()
                    .checked_mul(std::mem::size_of::<SnapshotRecord>())
                    .ok_or(ExportRecordError::Reservation)?,
            )
            .and_then(|n| {
                n.checked_add(
                    records
                        .reads
                        .capacity()
                        .checked_mul(std::mem::size_of::<ReadRecord>())?,
                )
            })
            .ok_or(ExportRecordError::Reservation)?;
        let remaining = records
            .max_bookkeeping_bytes
            .checked_sub(existing)
            .ok_or(ExportRecordError::Reservation)?;
        records.native_requests.prepare(remaining)
    }
    pub fn send_request_on_control(
        &self,
        source: &mut Option<crate::service::Request>,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.native_requests.send(source)
    }
    pub fn prepare_export_results_on_control(&self) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) || records.closed {
            return Err(ExportRecordError::Closed);
        }
        // Inline capacity ONE is already in the actual family constructor quote.
        records.native_export_result.prepare()
    }
    pub fn send_export_result_on_control(
        &self,
        source: &mut Option<crate::service::ExportEvent>,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) || records.closed {
            return Err(ExportRecordError::Closed);
        }
        let revision = records
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        records.native_export_result.send(source)?;
        records.revision = revision;
        Ok(())
    }
    /// No owning result escapes. The actual read completes before durable ACK.
    /// ```compile_fail
    /// fn escape(f: &hex_session::snapshot_exports::SessionExportFamily) {
    ///     let _ = f.read_export_result_on_control(|result| result.path());
    /// }
    /// ```
    pub fn read_export_result_on_control<R>(
        &self,
        read: impl for<'a> FnOnce(ExportResultRead<'a>) -> R,
    ) -> Result<Option<R>, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) || records.closed {
            return Err(ExportRecordError::Closed);
        }
        records.native_export_result.read(read)
    }
    pub fn receive_request_on_control(
        &self,
        destination: &mut Option<NativeRequestDelivery>,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let family = records.family;
        records.native_requests.receive(family, destination)
    }
    /// Shared-lock durable ACK destroys no owner, and cannot certify children.
    pub fn complete_request_on_control(
        &self,
        stamp: NativeRequestStamp,
    ) -> Result<(), ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        records.native_requests.complete(records.family, stamp)
    }
    pub fn insert_on_control(
        &self,
        key: &SnapshotProducerKey,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotStamp, ExportRecordError> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.insert_on_control(source)
    }
    pub fn reserve_read_on_control(
        &self,
        key: &SnapshotProducerKey,
        snapshot: SnapshotStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.reserve_read(snapshot)
    }
    /// Atomically root an already prepared owned worker view and reserve its
    /// initial Event read. Persistent registration and the caller's actual
    /// construction/union reservation must precede this call. Refusal leaves
    /// the exact pending view untouched. Success retires the record for cleanup
    /// after its initial/derived reads ACK; it exports no owning model/PCM.
    /// Transport binding is still fallible: retain the pending read on refusal
    /// and durably release it through the registered adapter before discarding.
    pub fn insert_event_read_on_control(
        &self,
        key: &SnapshotProducerKey,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        self.try_event_construction_on_control(key)?
            .insert_on_control(source)
    }

    /// Transfer already registered actual Event reads into the same fixed
    /// capacity-two family table. Full/Busy preserves ALL producer fields.
    pub fn enqueue_source_event_on_control(
        &self,
        key: &SnapshotProducerKey,
        canonical: &mut Option<SnapshotReadStamp>,
        device: &mut Option<SnapshotReadStamp>,
        tuning: &mut Option<SnapshotReadStamp>,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) || key.family != records.family {
            return Err(ExportRecordError::Closed);
        }
        records.enqueue_source_event(canonical, device, tuning)
    }
    /// Inspect the actual retained head before any UI receive effects. This
    /// scalar is correlation only; receive rechecks the actual SAME entry.
    pub fn source_event_head_on_control(
        &self,
    ) -> Result<Option<SourceEventHead>, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.source_event_head()
    }
    /// Borrow canonical facts/metadata of the SAME retained queue head. End
    /// this scope before receive, derive, ACK, submission or GPUI callbacks.
    pub fn read_source_event_head_on_control<R>(
        &self,
        read: impl for<'a> FnOnce(SourceEventHead, SnapshotRead<'a>) -> R,
    ) -> Result<Option<R>, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.read_source_event_head(read)
    }
    /// Reserve the requested extra canonical role BEFORE removing the head.
    /// Refusal retains it, including when durable ACKs have not yet collected.
    pub fn receive_source_event_on_control(
        &self,
        head: SourceEventHead,
        canonical_pending: bool,
        destination: &mut Option<SourceEventDelivery>,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.receive_source_event(head, canonical_pending, destination)
    }
    pub fn retire_on_control(
        &self,
        key: &SnapshotProducerKey,
        snapshot: SnapshotStamp,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        records.retire_on_control(snapshot)
    }
    pub fn read_with<R>(
        &self,
        stamp: &SnapshotReadStamp,
        read: impl for<'a> FnOnce(SnapshotRead<'a>) -> R,
    ) -> Result<R, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        records.read_with(stamp, read)
    }
    /// One inner guard covers both exact read incarnations and sealed binding.
    pub fn follows_source(
        &self,
        current: &SnapshotReadStamp,
        previous: &SnapshotReadStamp,
        generation: u64,
    ) -> Result<bool, ExportRecordError> {
        self.records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?
            .follows_source(current, previous, generation)
    }
    pub fn derive_read(
        &self,
        stamp: &SnapshotReadStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        self.records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?
            .derive_read(stamp)
    }
    /// A release cannot lose its final stamp just because control is processing
    /// another bounded bookkeeping step. Shared reads do not exclude rendering;
    /// a valid outstanding reader prevents any eligible owning destructor.
    /// No GUI response/native barrier or owning shutdown enqueue is involved.
    pub fn release_read(&self, stamp: SnapshotReadStamp) -> Result<(), ExportRecordError> {
        self.records
            .read()
            .map_err(|_| ExportRecordError::Stale)?
            .release_read(stamp)
    }
    /// Durable producer revocation; cleanup observes it even with lost wakeups.
    pub fn abandon_on_control(&self, key: &SnapshotProducerKey) -> Result<(), ExportRecordError> {
        if key.family != self.identity {
            return Err(ExportRecordError::Stale);
        }
        self.abandoned.store(true, Ordering::Release);
        Ok(())
    }
    /// Scoped actual guard, acquired BEFORE any short native publication barrier.
    /// The read surface exposes queries only. Actual joint inventory/seal work
    /// still needs its typed adapter inside this same protected scope.
    /// ```compile_fail
    /// use hex_session::snapshot_exports::SessionExportFamily;
    /// fn escape(family: &SessionExportFamily) {
    ///     let _escaped = family.inspect_queries_on_control(|view| view);
    /// }
    /// ```
    pub fn inspect_queries_on_control<R>(
        &self,
        read: impl for<'a> FnOnce(SnapshotFamilyQueries<'a>) -> R,
    ) -> Result<R, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        Ok(read(SnapshotFamilyQueries {
            records: &records,
            retiring_compiled: &self.retiring_compiled,
            retiring_request: &self.retiring_request,
            retiring_export_result: &self.retiring_export_result,
        }))
    }
    /// Called only by the already-registered persistent control adapter. One
    /// eligible snapshot owner Drop maximum; no guard waits for UI. Its caller
    /// must catch a destructor panic and freeze the exact family slot/charge.
    /// A pre-Drop latch prevents retries even outside that registered caller.
    pub fn cleanup_one_on_control(&mut self) -> Result<SnapshotFamilyCleanup, ExportRecordError> {
        if !self.retiring_snapshot.vacant() {
            return Err(ExportRecordError::Stale);
        }
        let mut records = match self.records.try_write() {
            Ok(records) => records,
            Err(TryLockError::WouldBlock) => return Ok(SnapshotFamilyCleanup::Pending),
            Err(TryLockError::Poisoned(_)) => return Err(ExportRecordError::Stale),
        };
        if self.abandoned.load(Ordering::Acquire) {
            records.abandon_source_events()?;
            records.close_on_control()?;
        }
        if !self.retiring_export_result.vacant() {
            return Err(ExportRecordError::Stale);
        }
        let family = records.family;
        if records.native_export_result.begin_cleanup(
            family,
            self.abandoned.load(Ordering::Acquire),
            &mut self.retiring_export_result,
        )? {
            drop(records);
            self.retiring_export_result.release();
            let records = self
                .records
                .get_mut()
                .map_err(|_| ExportRecordError::Stale)?;
            records
                .native_export_result
                .finish_cleanup(family, &mut self.retiring_export_result)?;
            return Ok(SnapshotFamilyCleanup::Pending);
        }
        if !self.retiring_request.vacant() {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            records.native_requests.close();
        }
        let family = records.family;
        if self.abandoned.load(Ordering::Acquire)
            && records
                .native_requests
                .begin_cleanup(family, &mut self.retiring_request)?
        {
            drop(records);
            self.retiring_request.release();
            let records = self
                .records
                .get_mut()
                .map_err(|_| ExportRecordError::Stale)?;
            records
                .native_requests
                .finish_cleanup(family, &mut self.retiring_request)?;
            return Ok(SnapshotFamilyCleanup::Pending);
        }
        if !self.retiring_compiled.vacant() {
            return Err(ExportRecordError::Stale);
        }
        if let Some(native) = &mut records.native_compiled {
            if self.abandoned.load(Ordering::Acquire) {
                native.close();
            }
            if native.begin_cleanup(&mut self.retiring_compiled)? {
                // Actual persistent destination, then release the family writer
                // BEFORE capability destruction. Outer &mut custody still holds.
                drop(records);
                self.retiring_compiled.release();
                let records = self
                    .records
                    .get_mut()
                    .map_err(|_| ExportRecordError::Stale)?;
                records
                    .native_compiled
                    .as_mut()
                    .ok_or(ExportRecordError::Stale)?
                    .finish_cleanup(&mut self.retiring_compiled)?;
                return Ok(SnapshotFamilyCleanup::Pending);
            }
        }
        records.collect_releases_on_control()?;
        if self.retiring_snapshot.begin(&mut records)? {
            drop(records);
            self.retiring_snapshot.release();
            let detached_records = self
                .records
                .get_mut()
                .map_err(|_| ExportRecordError::Stale)?;
            self.retiring_snapshot.finish(detached_records)?;
            // Only the original completion predicate follows normal disposal.
            // No owner destruction or admission occurs under this new guard.
            records = self
                .records
                .try_write()
                .map_err(|_| ExportRecordError::Stale)?;
        }
        if self.retiring_export_result.vacant()
            && records.native_export_result.vacant()
            && self.retiring_request.vacant()
            && records.native_requests.disposed()
            && self.retiring_compiled.vacant()
            && records
                .native_compiled
                .as_ref()
                .is_none_or(|native| native.disposed())
            && records.closed
            && records.snapshots.iter().all(|record| {
                record.snapshot.is_none()
                    && record.event_metadata.is_none()
                    && !record.cleanup_started
            })
            && records
                .reads
                .iter()
                .all(|read| !read.occupied.load(Ordering::Acquire))
        {
            Ok(SnapshotFamilyCleanup::Disposed)
        } else {
            Ok(SnapshotFamilyCleanup::Pending)
        }
    }
}
/// No records/owners are extractable, including through a held seal scope.
pub struct SnapshotFamilyQueries<'a> {
    records: &'a SnapshotExportRecords,
    retiring_compiled: &'a native_transport::CompiledRetirement,
    retiring_request: &'a native_requests::RequestRetirement,
    retiring_export_result: &'a native_export_results::ExportResultRetirement,
}
impl SnapshotFamilyQueries<'_> {
    /// Actual nested records while the enclosing family read guard stays held.
    /// The enclosing holder/table and every other root join the SAME ledger;
    /// this neither supplies a construction quote nor authorizes publication.
    pub fn visit_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.records.visit_selected_storage_on_control(ledger)?;
        self.retiring_compiled.visit(ledger)?;
        self.retiring_request.visit()?;
        self.retiring_export_result.visit(ledger)
    }

    pub fn inventory_revision(&self) -> u64 {
        self.records.inventory_revision()
    }
    pub fn snapshot_count(&self) -> usize {
        self.records
            .snapshots
            .iter()
            .filter(|record| record.snapshot.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> SnapshotExportReservation {
        SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        }
    }
    fn pending(revision: u64) -> Option<ViewState> {
        let session = crate::Session::new(48_000, 8, 1024).unwrap();
        Some(crate::service::view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            1,
            false,
            None,
        ))
    }
    #[test]
    fn actual_family_old_read_survives_abandon_until_durable_release_and_control_drop() {
        let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut source = pending(3);
        let snapshot = family.insert_on_control(&producer, &mut source).unwrap();
        let read = family.reserve_read_on_control(&producer, snapshot).unwrap();
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        assert_eq!(family.read_with(&read, |view| view.revision()), Ok(3));
        let derived = family.derive_read(&read).unwrap();
        family.release_read(read).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        assert_eq!(family.read_with(&derived, |view| view.revision()), Ok(3));
        family.release_read(derived).unwrap();
        assert_eq!(
            family.inspect_queries_on_control(|view| view.snapshot_count()),
            Ok(1)
        );
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
        assert_eq!(
            family.inspect_queries_on_control(|view| view.snapshot_count()),
            Ok(0)
        );
        assert!(matches!(
            family.reserve_read_on_control(&producer, snapshot),
            Err(ExportRecordError::Closed)
        ));
    }
    #[test]
    fn actual_family_reclaimed_record_reuse_preserves_generation_and_new_source() {
        let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut old = pending(10);
        let first = family.insert_on_control(&producer, &mut old).unwrap();
        family.retire_on_control(&producer, first).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        let mut next = pending(11);
        let second = family.insert_on_control(&producer, &mut next).unwrap();
        assert_eq!(first.index, second.index);
        assert_ne!(first.generation, second.generation);
        assert!(matches!(
            family.reserve_read_on_control(&producer, first),
            Err(ExportRecordError::Stale)
        ));
        let read = family.reserve_read_on_control(&producer, second).unwrap();
        assert_eq!(family.read_with(&read, |view| view.revision()), Ok(11));
        family.release_read(read).unwrap();
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
    }
    #[test]
    fn actual_family_guard_refuses_insert_and_abandon_is_durable_without_waiting_for_it() {
        let (family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let (other, other_producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut source = pending(5);
        assert_eq!(
            family.insert_on_control(&other_producer, &mut source),
            Err(ExportRecordError::Stale)
        );
        family
            .inspect_queries_on_control(|view| {
                let revision = view.inventory_revision();
                assert_eq!(
                    family.insert_on_control(&producer, &mut source),
                    Err(ExportRecordError::Busy)
                );
                assert!(source.is_some());
                family.abandon_on_control(&producer).unwrap();
                assert_eq!(view.inventory_revision(), revision);
                assert_eq!(view.snapshot_count(), 0);
            })
            .unwrap();
        assert_eq!(
            family.insert_on_control(&producer, &mut source),
            Err(ExportRecordError::Closed)
        );
        assert!(source.is_some());
        assert_eq!(
            other.abandon_on_control(&producer),
            Err(ExportRecordError::Stale)
        );
    }
}

#[cfg(test)]
mod inventory_guard_tests {
    use super::*;
    #[test]
    fn structural_snapshot_guard_blocks_insertion_but_preserves_durable_ack_and_abandonment() {
        let (mut family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 2,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let session = crate::Session::new(48_000, 8, 1024).unwrap();
        let view = |revision| {
            crate::service::view_state_owned(
                &session,
                hex_engine::empty_midi_preview_on_control(),
                revision,
                1,
                false,
                None,
            )
        };
        let snapshot = family
            .insert_on_control(&producer, &mut Some(view(7)))
            .unwrap();
        let read = family.reserve_read_on_control(&producer, snapshot).unwrap();
        let mut refused = Some(view(8));
        let address = std::ptr::from_ref(&refused.as_ref().unwrap().project);
        let guard = family.try_inventory_guard_on_control().unwrap();
        let revision = guard.inventory_revision();
        assert_eq!(guard.snapshot_count(), 1);
        assert_eq!(
            family.insert_on_control(&producer, &mut refused),
            Err(ExportRecordError::Busy)
        );
        assert_eq!(
            address,
            std::ptr::from_ref(&refused.as_ref().unwrap().project)
        );
        assert_eq!(
            family.read_with(&read, |snapshot| snapshot.revision()),
            Ok(7)
        );
        family.release_read(read).unwrap();
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(guard.inventory_revision(), revision);
        assert_eq!(
            guard.snapshot_count(),
            1,
            "ACK does not remove the structural owner"
        );
        assert!(
            family.abandoned.load(Ordering::Acquire),
            "inner guard is not an atomic cancellation freeze"
        );
        drop(guard);
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
        assert!(refused.is_some());
    }
}

#[cfg(test)]
mod source_retirement_tests {
    use super::*;
    #[test]
    fn actual_source_metadata_cleanup_unwind_keeps_same_destination_and_never_retries() {
        let (mut family, key) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 4,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let session = crate::Session::new(48_000, 1, 1024).unwrap();
        let mut view = Some(crate::service::view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            7,
            1,
            false,
            None,
        ));
        let mut metadata = Some(super::source_event::SourceEventMetadata {
            facts: super::SourceEventFacts::default(),
            status: Some("still owned".into()),
            error: None,
            audio_batch_failure: None,
            gesture_ack: None,
            tuning_gesture_ack: None,
            tuning_library: None,
        });
        let read = family
            .try_event_construction_on_control(&key)
            .unwrap()
            .insert_source_on_control(&mut view, &mut metadata)
            .unwrap();
        family.abandon_on_control(&key).unwrap();
        family.release_read(read).unwrap();
        family.retiring_snapshot.panic_after_view = true;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            family.cleanup_one_on_control()
        }))
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"injected snapshot boundary after view disposal")
        );
        assert!(family.retiring_snapshot.retains_metadata());
        assert!(matches!(
            family.try_inventory_guard_on_control(),
            Err(ExportRecordError::Stale)
        ));
        assert_eq!(
            family.cleanup_one_on_control(),
            Err(ExportRecordError::Stale)
        );
        assert!(family.retiring_snapshot.retains_metadata());
        assert!(
            !family.records.is_poisoned(),
            "destruction was outside the inner writer"
        );
    }

    #[test]
    fn concrete_candidate_queue_profile_shape_is_measured_inside_unchanged_bookkeeping_limit() {
        // Measurement only, NOT the final producer/receiver admission profile.
        // UI36 + independent worker-current1 + queued6 + producer3 + UI-head3.
        let snapshots = 36 + 1 + 2 * 3 + 3 + 3;
        // UI36 + queued6 + producer3 + bounded in-flight/late derivations4.
        let reads = 36 + 2 * 3 + 3 + 4;
        let limits = SnapshotExportReservation {
            snapshot_records: snapshots,
            read_records: reads,
            max_bookkeeping_bytes: 65_536,
        };
        let quote = SnapshotExportRecords::reservation_quote(limits).unwrap();
        let (family, _) = SessionExportFamily::try_new_native_on_control(limits).unwrap();
        family.prepare_requests_on_control().unwrap();
        let records = family.records.try_read().unwrap();
        let actual = std::mem::size_of::<SessionExportFamily>()
            + records.snapshots.capacity() * std::mem::size_of::<SnapshotRecord>()
            + records.reads.capacity() * std::mem::size_of::<ReadRecord>()
            + super::native_requests::NativeRequests::requested_backing().unwrap();
        println!(
            "MEASUREMENT_ONLY snapshot_records={snapshots} read_records={reads} record_body={} read_body={} family_body={} table_quote={quote} total_actual_with_requests={actual} unchanged_limit=65536",
            std::mem::size_of::<SnapshotRecord>(),
            std::mem::size_of::<ReadRecord>(),
            std::mem::size_of::<SessionExportFamily>()
        );
        assert!(actual <= limits.max_bookkeeping_bytes);
    }
}

#[cfg(test)]
mod nonstatic_vec_tests {
    use super::*;
    #[test]
    fn actual_nonstatic_snapshot_guard_vec_backing_is_measured_without_owner_escape() {
        let (family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 2,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let session = crate::Session::new(48_000, 8, 1024).unwrap();
        let view = crate::service::view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            7,
            1,
            false,
            None,
        );
        family
            .insert_on_control(&producer, &mut Some(view))
            .unwrap();
        let mut guards = Vec::with_capacity(3);
        guards.push(family.try_inventory_guard_on_control().unwrap());
        let budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 8,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        };
        let measure = |budget| -> Result<_, hex_engine::AudioStorageError> {
            let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(budget)?;
            ledger.borrow_metadata_vec_backing_on_control(&guards)?;
            ledger.borrow_metadata_vec_backing_on_control(&guards)?;
            let summary = ledger.summary()?;
            assert_eq!(
                summary.data_bytes,
                guards.capacity() * std::mem::size_of::<SnapshotFamilyInventoryGuard<'_>>()
            );
            assert_eq!(summary.data_allocations, 1);
            assert_eq!(guards[0].snapshot_count(), 1);
            assert!(summary.require_complete().is_err());
            Ok(summary)
        };
        let measured = measure(budget).unwrap();
        let mut exact = budget;
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(measure(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(measure(exact).is_err());
        if hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!(
                "SUPPORTED actual nonstatic Session guard Vec and separately traversed snapshot children"
            );
            let mut ledger =
                hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
            ledger
                .borrow_metadata_vec_backing_on_control(&guards)
                .unwrap();
            guards[0]
                .visit_selected_storage_on_control(&mut ledger)
                .unwrap();
            assert!(
                ledger.summary().unwrap().data_bytes > measured.data_bytes,
                "guard references do not certify or silently charge nested owned data"
            );
        }
    }
}

#[cfg(test)]
mod request_budget_tests {
    use super::super::native_requests::NativeRequests;
    use super::*;
    type Error = ExportRecordError;
    #[test]
    fn native_request_family_budget_includes_both_existing_backings_before_request_allocation() {
        let base = std::mem::size_of::<SessionExportFamily>()
            + 2 * std::mem::size_of::<SnapshotRecord>()
            + 2 * std::mem::size_of::<ReadRecord>();
        let request_bytes = NativeRequests::requested_backing().unwrap();
        let exact = base.checked_add(request_bytes).unwrap();
        for (limit, expected) in [(exact - 1, Err(Error::Reservation)), (exact, Ok(()))] {
            let (mut family, key) =
                SessionExportFamily::try_new_native_on_control(SnapshotExportReservation {
                    snapshot_records: 2,
                    read_records: 2,
                    max_bookkeeping_bytes: limit,
                })
                .unwrap();
            assert_eq!(family.prepare_requests_on_control(), expected);
            {
                let records = family.records.read().unwrap();
                assert_eq!(records.snapshots.capacity(), 2);
                assert_eq!(records.reads.capacity(), 2);
                assert_eq!(records.native_requests.ready(), expected.is_ok());
            }
            if expected.is_ok() {
                assert_eq!(family.prepare_requests_on_control(), Err(Error::Stale));
            }
            family.abandon_on_control(&key).unwrap();
            assert_eq!(
                family.cleanup_one_on_control(),
                Ok(SnapshotFamilyCleanup::Disposed)
            );
        }
        eprintln!(
            "SUPPORTED actual native family inline body+snapshot/read backings+request backing exact={exact}; minus1 refuses before request allocation (base family already exists)"
        );
    }
}

#[cfg(test)]
#[path = "native_export_results/tests.rs"]
mod native_export_result_tests;
