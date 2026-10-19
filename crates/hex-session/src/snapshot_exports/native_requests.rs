//! Concrete registered Service request storage, under the existing family lock.
//! A queued or claimed payload never becomes an admission/completeness flag.
use super::{ExportRecordError as Error, *};
use crate::service::Request;

pub(crate) const CAPACITY: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Vacant,
    Queued,
    Claimed,
    Retiring,
}
struct Record {
    generation: u64,
    sequence: u64,
    phase: Phase,
    payload: Option<Request>,
    completed: AtomicBool,
    cleanup_pending: bool,
}
/// Correlation of an actual claimed field. Not a source/cleanup permit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRequestStamp {
    family: u64,
    index: usize,
    generation: u64,
}
/// Actual worker-local destination of the guarded fixed transfer. Its private
/// stamp stays paired with the payload until Service destructures the request.
pub struct NativeRequestDelivery {
    pub(crate) request: Request,
    pub(crate) stamp: Option<NativeRequestStamp>,
}
impl NativeRequestDelivery {
    pub(crate) fn unqualified(request: Request) -> Self {
        Self {
            request,
            stamp: None,
        }
    }
}
#[derive(Default)]
pub(super) struct NativeRequests {
    records: Vec<Record>,
    sequence: u64,
    closed: bool,
}
impl NativeRequests {
    pub(super) fn requested_backing() -> Result<usize, Error> {
        CAPACITY
            .checked_mul(std::mem::size_of::<Record>())
            .ok_or(Error::Reservation)
    }
    pub(super) fn prepare(&mut self, remaining: usize) -> Result<(), Error> {
        if self.closed || !self.records.is_empty() {
            return Err(Error::Stale);
        }
        let bytes = Self::requested_backing()?;
        if bytes > remaining || self.records.capacity() != 0 {
            return Err(Error::Reservation);
        }
        self.records
            .try_reserve_exact(CAPACITY)
            .map_err(|_| Error::Allocation)?;
        // Pinned Global/Vec requested layout is quoted above. Any unexpected
        // actual capacity refuses before a request/worker can escape.
        if self.records.capacity() != CAPACITY {
            return Err(Error::Reservation);
        }
        self.records.resize_with(CAPACITY, || Record {
            generation: 0,
            sequence: 0,
            phase: Phase::Vacant,
            payload: None,
            completed: AtomicBool::new(false),
            cleanup_pending: false,
        });
        Ok(())
    }
    pub(super) fn ready(&self) -> bool {
        !self.closed && self.records.len() == CAPACITY
    }
    fn collect_completed(&mut self) -> Result<(), Error> {
        for record in &mut self.records {
            if record.completed.load(Ordering::Acquire) {
                if record.phase != Phase::Claimed
                    || record.payload.is_some()
                    || record.cleanup_pending
                {
                    return Err(Error::Stale);
                }
                record.phase = Phase::Vacant;
                record.completed.store(false, Ordering::Relaxed);
            }
        }
        Ok(())
    }
    fn send_destination(&mut self) -> Result<(usize, u64, u64), Error> {
        if !self.ready() {
            return Err(Error::Closed);
        }
        self.collect_completed()?;
        let (index, record) = self
            .records
            .iter()
            .enumerate()
            .find(|(_, r)| r.phase == Phase::Vacant)
            .ok_or(Error::Reservation)?;
        if record.payload.is_some() || record.cleanup_pending {
            return Err(Error::Stale);
        }
        Ok((
            index,
            record.generation.checked_add(1).ok_or(Error::Exhausted)?,
            self.sequence.checked_add(1).ok_or(Error::Exhausted)?,
        ))
    }
    fn install(&mut self, index: usize, generation: u64, sequence: u64, request: Request) {
        let record = &mut self.records[index];
        record.payload = Some(request);
        record.generation = generation;
        record.sequence = sequence;
        record.phase = Phase::Queued;
        self.sequence = sequence;
    }
    pub(super) fn send(&mut self, source: &mut Option<Request>) -> Result<(), Error> {
        if source.is_none() {
            return Err(Error::Stale);
        }
        let (index, generation, sequence) = self.send_destination()?;
        if let Some(request) = source.take() {
            self.install(index, generation, sequence, request);
        }
        Ok(())
    }
    pub(super) fn receive(
        &mut self,
        family: u64,
        destination: &mut Option<NativeRequestDelivery>,
    ) -> Result<(), Error> {
        if !self.ready() {
            return Err(Error::Closed);
        }
        if destination.is_some() {
            return Err(Error::Stale);
        }
        self.collect_completed()?;
        let (index, record) = self
            .records
            .iter_mut()
            .enumerate()
            .filter(|(_, r)| r.phase == Phase::Queued)
            .min_by_key(|(_, r)| r.sequence)
            .ok_or(Error::Reservation)?;
        if record.cleanup_pending || record.payload.is_none() {
            return Err(Error::Stale);
        }
        let stamp = NativeRequestStamp {
            family,
            index,
            generation: record.generation,
        };
        // All fallible validation ends before actual fixed movement. A claimed
        // slot remains occupied until the actual worker's explicit completion.
        if let Some(request) = record.payload.take() {
            *destination = Some(NativeRequestDelivery {
                request,
                stamp: Some(stamp),
            });
            record.phase = Phase::Claimed;
        }
        Ok(())
    }
    pub(super) fn complete(&self, family: u64, stamp: NativeRequestStamp) -> Result<(), Error> {
        if family != stamp.family {
            return Err(Error::Stale);
        }
        let record = self.records.get(stamp.index).ok_or(Error::Stale)?;
        if record.generation != stamp.generation
            || record.phase != Phase::Claimed
            || record.payload.is_some()
            || record.cleanup_pending
        {
            return Err(Error::Stale);
        }
        // Durable nonowning completion, like the existing snapshot-read ACK.
        // It destroys no payload/owner and grants no publication authority.
        record
            .completed
            .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
            .map_err(|_| Error::Stale)?;
        Ok(())
    }
    pub(super) fn close(&mut self) {
        self.closed = true;
    }
    pub(super) fn begin_cleanup(
        &mut self,
        family: u64,
        destination: &mut RequestRetirement,
    ) -> Result<bool, Error> {
        if !destination.vacant() {
            return Err(Error::Stale);
        }
        self.collect_completed()?;
        let Some((index, record)) = self
            .records
            .iter_mut()
            .enumerate()
            .find(|(_, r)| r.phase == Phase::Queued)
        else {
            return Ok(false);
        };
        if record.cleanup_pending || record.payload.is_none() {
            return Err(Error::Stale);
        }
        destination.stamp = Some(NativeRequestStamp {
            family,
            index,
            generation: record.generation,
        });
        destination.payload = record.payload.take();
        record.phase = Phase::Retiring;
        record.cleanup_pending = true;
        Ok(true)
    }
    pub(super) fn finish_cleanup(
        &mut self,
        family: u64,
        destination: &mut RequestRetirement,
    ) -> Result<(), Error> {
        if destination.pending || destination.payload.is_some() {
            return Err(Error::Stale);
        }
        let stamp = destination.stamp.ok_or(Error::Stale)?;
        if family != stamp.family {
            return Err(Error::Stale);
        }
        let record = self.records.get_mut(stamp.index).ok_or(Error::Stale)?;
        if record.generation != stamp.generation
            || record.phase != Phase::Retiring
            || !record.cleanup_pending
            || record.payload.is_some()
        {
            return Err(Error::Stale);
        }
        record.phase = Phase::Vacant;
        record.cleanup_pending = false;
        destination.stamp = None;
        Ok(())
    }
    pub(super) fn disposed(&self) -> bool {
        self.records
            .iter()
            .all(|r| r.phase == Phase::Vacant && r.payload.is_none() && !r.cleanup_pending)
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        ledger.borrow_metadata_vec_backing_on_control(&self.records)?;
        for record in &self.records {
            if record.cleanup_pending {
                return Err(hex_engine::AudioStorageError::UnknownStorage);
            }
            match (record.phase, &record.payload) {
                (Phase::Vacant, None) => {}
                (Phase::Claimed, None) if record.completed.load(Ordering::Acquire) => {}
                (Phase::Queued, Some(request)) => visit_request(request, ledger)?,
                // A still-claimed worker local is NOT an empty root. Actual
                // dispatch fields must own its products before explicit ACK.
                _ => return Err(hex_engine::AudioStorageError::UnknownStorage),
            }
        }
        Ok(())
    }
}
fn visit_request<'a>(
    request: &'a Request,
    ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
) -> Result<(), hex_engine::AudioStorageError> {
    use hex_engine::AudioStorageError::UnknownStorage;
    match request {
        // These exact enum variants have only inline scalar/Copy fields. The
        // parent request Vec already charges every byte, including spare slots.
        Request::Undo
        | Request::Redo
        | Request::SaveCurrent
        | Request::RestoreStoppedPlayback
        | Request::RestartAudio { .. }
        | Request::ArmGainRecording { .. }
        | Request::DropBrowserDeviceOnArrangement { .. }
        | Request::DropNativeDevice { .. }
        | Request::SetDeviceParameter { .. }
        | Request::ConvertClip { .. }
        | Request::CopyTimeRange(_)
        | Request::CutTimeRange(_)
        | Request::PasteTimeRange { .. }
        | Request::InsertTimeRange { .. }
        | Request::RestorePlayback { .. }
        | Request::NewWithAudioMode { .. }
        | Request::DeviceGesture(_)
        | Request::TuningGesture(_) => Ok(()),
        Request::SaveDevicePreset { path, .. }
        | Request::LoadDevicePreset { path, .. }
        | Request::Import(path)
        | Request::Open(path)
        | Request::SaveNew(path)
        | Request::Export(path)
        | Request::ImportToTrack { path, .. }
        | Request::ImportToTrackWithOverwrite { path, .. }
        | Request::ImportToNewTrack { path, .. }
        | Request::ImportToSession { path, .. } => {
            ledger.borrow_metadata_path_backing_on_control(path)
        }
        Request::CopyClips(ids) => ledger.borrow_metadata_vec_backing_on_control(ids),
        // No type/phase/counter upgrades raw input Arcs, opaque model commands,
        // boxed device IDs, relink BTreeMaps or export controls. Their actual
        // owning producer modules still need constructor/buffer visitors.
        Request::PrepareTuningAudition { .. }
        | Request::PrepareLiveExpression { .. }
        | Request::PrepareComputerKeyboard { .. }
        | Request::GainLane { .. }
        | Request::TuningLibrary { .. }
        | Request::AddNativeDevice { .. }
        | Request::ReplaceNativeDevice { .. }
        | Request::ImportAudioBatch { .. }
        | Request::OpenWithAudioMode { .. }
        | Request::OpenWith { .. }
        | Request::Edit(_)
        | Request::ExportRange { .. } => Err(UnknownStorage),
    }
}

#[derive(Default)]
pub(super) struct RequestRetirement {
    stamp: Option<NativeRequestStamp>,
    payload: Option<Request>,
    pending: bool,
}
impl RequestRetirement {
    pub(super) fn vacant(&self) -> bool {
        self.stamp.is_none() && self.payload.is_none() && !self.pending
    }
    pub(super) fn release(&mut self) {
        self.pending = true;
        drop(self.payload.take());
        self.pending = false;
    }
    pub(super) fn visit(&self) -> Result<(), hex_engine::AudioStorageError> {
        if !self.vacant() {
            Err(hex_engine::AudioStorageError::UnknownStorage)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_gesture::{
        DeviceParameterSnapshot, GestureKey, GestureRequest, ParameterId, ParameterValue,
    };
    fn terminal() -> GestureRequest {
        GestureRequest::CommitSnapshot {
            key: GestureKey {
                generation: 3,
                token: 9,
            },
            sequence: 17,
            snapshot: DeviceParameterSnapshot::new(&[(
                ParameterId::from_static(1),
                ParameterValue::Float(0.5),
            )])
            .unwrap(),
        }
    }
    #[test]
    fn native_request_actual_32_slots_keep_inline_terminal_and_fifo_through_claim_ack() {
        fn sync<T: Send + Sync>() {}
        sync::<NativeRequests>();
        let mut queue = NativeRequests::default();
        let bytes = NativeRequests::requested_backing().unwrap();
        assert_eq!(queue.prepare(bytes - 1), Err(Error::Reservation));
        assert_eq!(queue.records.capacity(), 0);
        queue.prepare(bytes).unwrap();
        assert_eq!(queue.records.capacity(), 32);
        let terminal = terminal();
        for _ in 0..32 {
            let mut source = Some(Request::DeviceGesture(terminal));
            queue.send(&mut source).unwrap();
            assert!(source.is_none());
        }
        let mut source = Some(Request::DeviceGesture(terminal));
        assert_eq!(queue.send(&mut source), Err(Error::Reservation));
        assert!(matches!(source,Some(Request::DeviceGesture(value)) if value==terminal));
        let mut delivery = None;
        queue.receive(7, &mut delivery).unwrap();
        let NativeRequestDelivery { request, stamp } = delivery.take().unwrap();
        assert!(matches!(request,Request::DeviceGesture(value) if value==terminal));
        let stamp = stamp.unwrap();
        assert_eq!(stamp.index, 0);
        assert_eq!(queue.send(&mut source), Err(Error::Reservation)); // claimed is NOT vacant
        queue.complete(7, stamp).unwrap();
        assert_eq!(queue.complete(7, stamp), Err(Error::Stale));
        queue.send(&mut source).unwrap();
        assert!(source.is_none());
        assert_eq!(queue.complete(7, stamp), Err(Error::Stale)); // generation reuse
        for index in 1..32 {
            queue.receive(7, &mut delivery).unwrap();
            let NativeRequestDelivery { request, stamp } = delivery.take().unwrap();
            assert!(matches!(request,Request::DeviceGesture(value) if value==terminal));
            let stamp = stamp.unwrap();
            assert_eq!(stamp.index, index);
            queue.complete(7, stamp).unwrap();
        }
        queue.receive(7, &mut delivery).unwrap();
        let NativeRequestDelivery { request, stamp } = delivery.take().unwrap();
        assert!(matches!(request,Request::DeviceGesture(value) if value==terminal));
        assert_eq!(stamp.unwrap().index, 0);
        queue.complete(7, stamp.unwrap()).unwrap();
        assert_eq!(queue.receive(7, &mut delivery), Err(Error::Reservation));
        assert!(queue.disposed());
        eprintln!(
            "SUPPORTED actual native request32 inline terminal/FIFO/claimed-full/durableACK/stale generation; backing exact={bytes} minus1 capacity0"
        );
    }
    #[test]
    fn native_request_claim_unknown_and_cancelled_queue_cleanup_keep_real_payloads() {
        let mut queue = NativeRequests::default();
        queue
            .prepare(NativeRequests::requested_backing().unwrap())
            .unwrap();
        for _ in 0..2 {
            queue.send(&mut Some(Request::Undo)).unwrap();
        }
        let mut destination = None;
        queue.receive(11, &mut destination).unwrap();
        let NativeRequestDelivery { request, stamp } = destination.take().unwrap();
        assert!(matches!(request, Request::Undo));
        assert_eq!(queue.complete(12, stamp.unwrap()), Err(Error::Stale));
        let budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
        assert_eq!(
            queue.visit(&mut ledger),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        );
        drop(ledger);
        queue.close();
        let mut producer = Some(Request::Redo);
        assert_eq!(queue.send(&mut producer), Err(Error::Closed));
        assert!(matches!(producer, Some(Request::Redo)));
        let mut retirement = RequestRetirement::default();
        assert!(queue.begin_cleanup(11, &mut retirement).unwrap());
        assert!(matches!(retirement.payload, Some(Request::Undo)));
        retirement.release();
        queue.finish_cleanup(11, &mut retirement).unwrap();
        assert!(!queue.disposed()); // actual claimed worker has NOT completed
        assert!(!queue.begin_cleanup(11, &mut retirement).unwrap());
        queue.complete(11, stamp.unwrap()).unwrap();
        queue.collect_completed().unwrap();
        assert!(queue.disposed());
        eprintln!(
            "SUPPORTED request claimed destination/queued payload are not empty roots; wrong family refuses; cancel retains producer; actual queued cleanup separate from worker ACK"
        );
    }
    #[test]
    fn native_request_existing_family_guard_refuses_send_and_accounts_actual_backing() {
        let (mut family, key) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 2,
                max_bookkeeping_bytes: 65_536,
            })
            .unwrap();
        family.prepare_requests_on_control().unwrap();
        let guard = family.try_inventory_guard_on_control().unwrap();
        let mut source = Some(Request::Undo);
        assert_eq!(
            family.send_request_on_control(&mut source),
            Err(Error::Busy)
        );
        assert!(source.is_some());
        let budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
        guard
            .visit_selected_storage_on_control(&mut ledger)
            .unwrap();
        let empty = ledger.summary().unwrap();
        assert!(empty.data_bytes >= NativeRequests::requested_backing().unwrap());
        drop(ledger);
        drop(guard);
        family.send_request_on_control(&mut source).unwrap();
        assert!(source.is_none());
        let mut delivery = None;
        family.receive_request_on_control(&mut delivery).unwrap();
        let NativeRequestDelivery { request, stamp } = delivery.take().unwrap();
        assert!(matches!(request, Request::Undo));
        // Durable ACK coexists with the held shared guard (no new lock).
        let guard = family.try_inventory_guard_on_control().unwrap();
        family.complete_request_on_control(stamp.unwrap()).unwrap();
        drop(guard);
        family.abandon_on_control(&key).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
        eprintln!(
            "SUPPORTED actual registered-family request backing + shared guard Busy/preserved producer/ACK; no owning native playback admission claimed"
        );
    }
}
