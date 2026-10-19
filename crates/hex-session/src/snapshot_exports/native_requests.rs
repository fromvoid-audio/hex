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
    #[cfg(test)]
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        ledger.borrow_sampler_family_requests_on_control(self)
    }
}
impl hex_engine::sampler_host_fields::FamilyRequestRecords for NativeRequests {
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self)
    }
    fn slots(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        let Self {
            records,
            sequence: _,
            closed: _,
        } = self;
        hex_engine::sampler_host_fields::VecRef::of(records)
    }
    fn request(
        &self,
        index: usize,
    ) -> Result<
        hex_engine::sampler_host_fields::FamilyRequestFields<'_>,
        hex_engine::AudioStorageError,
    > {
        use hex_engine::{
            AudioStorageError::UnknownStorage,
            sampler_host_fields::{BodyRef, FamilyRequestFields},
        };
        let record = self
            .records
            .get(index)
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
        let Record {
            generation: _,
            sequence: _,
            phase,
            payload,
            completed,
            cleanup_pending,
        } = record;
        if *cleanup_pending {
            return Err(UnknownStorage);
        }
        let request = match (phase, payload) {
            (Phase::Vacant, None) => None,
            (Phase::Claimed, None) if completed.load(Ordering::Acquire) => None,
            (Phase::Queued, Some(request)) => Some(request_fields(request)?),
            _ => return Err(UnknownStorage),
        };
        Ok(FamilyRequestFields {
            body: BodyRef::of(record),
            request,
        })
    }
}
fn request_fields(
    request: &Request,
) -> Result<hex_engine::sampler_host_fields::FamilyRequestPayload<'_>, hex_engine::AudioStorageError>
{
    use hex_engine::{
        AudioStorageError::UnknownStorage,
        sampler_host_fields::{BodyRef, FamilyRequestPayload},
    };
    // Borrow-only Copy assertions: no 800-byte snapshot/tuple temporary. A new
    // owning field cannot silently remain in the supported inline category.
    fn copy_field<T: Copy>(_: &T) {}
    let body = BodyRef::of(request);
    macro_rules! inline { ($($field:expr),* $(,)?) => {{ $(copy_field($field);)* Ok(FamilyRequestPayload::Inline { body }) }} }
    match request {
        Request::Undo | Request::Redo | Request::SaveCurrent | Request::RestoreStoppedPlayback => {
            inline!()
        }
        Request::RestartAudio {
            project_generation,
            request_id,
        } => inline!(project_generation, request_id),
        Request::ArmGainRecording {
            project_generation,
            revision,
            track,
        } => inline!(project_generation, revision, track),
        Request::DropBrowserDeviceOnArrangement {
            project_generation,
            revision,
            class,
            type_id,
            destination,
        } => inline!(project_generation, revision, class, type_id, destination),
        Request::DropNativeDevice {
            project_generation,
            revision,
            source,
            track,
            before,
            replace,
        } => inline!(project_generation, revision, source, track, before, replace),
        Request::SetDeviceParameter {
            device,
            parameter,
            value,
        } => inline!(device, parameter, value),
        Request::ConvertClip {
            project_generation,
            revision,
            clip,
            to_audio,
        } => inline!(project_generation, revision, clip, to_audio),
        Request::CopyTimeRange(range) | Request::CutTimeRange(range) => {
            Ok(FamilyRequestPayload::TimeRange { body, range })
        }
        Request::PasteTimeRange { track, frame } | Request::InsertTimeRange { track, frame } => {
            inline!(track, frame)
        }
        Request::RestorePlayback { generation } => inline!(generation),
        Request::NewWithAudioMode { mode, reservation } => inline!(mode, reservation),
        Request::DeviceGesture(value) => inline!(value),
        Request::TuningGesture(value) => inline!(value),
        Request::SaveDevicePreset { owner, path } | Request::LoadDevicePreset { owner, path } => {
            copy_field(owner);
            Ok(FamilyRequestPayload::Path { body, path })
        }
        Request::Import(path)
        | Request::Open(path)
        | Request::SaveNew(path)
        | Request::Export(path) => Ok(FamilyRequestPayload::Path { body, path }),
        Request::ImportToTrack { path, track, start }
        | Request::ImportToTrackWithOverwrite { path, track, start } => {
            copy_field(track);
            copy_field(start);
            Ok(FamilyRequestPayload::Path { body, path })
        }
        Request::ImportToNewTrack {
            path,
            before_track,
            start,
        } => {
            copy_field(before_track);
            copy_field(start);
            Ok(FamilyRequestPayload::Path { body, path })
        }
        Request::ImportToSession { path, track, scene } => {
            copy_field(track);
            copy_field(scene);
            Ok(FamilyRequestPayload::Path { body, path })
        }
        Request::CopyClips(ids) => Ok(FamilyRequestPayload::ClipIds { body, ids }),
        Request::PrepareTuningAudition {
            revision: _,
            input: _,
        }
        | Request::PrepareLiveExpression {
            input: _,
            revision: _,
        }
        | Request::PrepareComputerKeyboard {
            project_generation: _,
            revision: _,
            request_id: _,
            track: _,
            input: _,
        }
        | Request::GainLane {
            project_generation: _,
            revision: _,
            track: _,
            edit: _,
        }
        | Request::TuningLibrary {
            project_generation: _,
            revision: _,
            operation_id: _,
            operation: _,
        }
        | Request::AddNativeDevice {
            track: _,
            class: _,
            type_id: _,
            before: _,
        }
        | Request::ReplaceNativeDevice {
            device: _,
            class: _,
            type_id: _,
        }
        | Request::ImportAudioBatch {
            paths: _,
            destination: _,
            context: _,
            control: _,
        }
        | Request::OpenWithAudioMode {
            source: _,
            relinks: _,
            mode: _,
            reservation: _,
        }
        | Request::OpenWith {
            source: _,
            relinks: _,
        }
        | Request::Edit(_)
        | Request::ExportRange {
            path: _,
            start: _,
            frames: _,
            signal: _,
            control: _,
            overwrite: _,
        } => Err(UnknownStorage),
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
    fn native_request_time_range_actual_capacity_dedup_and_exact_limits() {
        use hex_engine::{
            AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
        };
        use hex_project::{Command, Created, Editor, Project, TimeRange, TrackId};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "range".into(),
            })
            .unwrap()
        else {
            panic!("actual track")
        };
        let budget = AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut queue = NativeRequests::default();
        queue
            .prepare(NativeRequests::requested_backing().unwrap())
            .unwrap();
        let summary = |budget| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            queue.visit(&mut ledger)?;
            ledger.summary()
        };
        let empty = summary(budget).unwrap();
        let mut actual_backing = 0;
        for cut in [false, true] {
            let mut tracks = Vec::with_capacity(128);
            tracks.push(track);
            assert_eq!(tracks.capacity(), 128);
            actual_backing += tracks.capacity() * std::mem::size_of::<TrackId>();
            let range = TimeRange::new(tracks, 0, 64).unwrap();
            queue
                .send(&mut Some(if cut {
                    Request::CutTimeRange(range)
                } else {
                    Request::CopyTimeRange(range)
                }))
                .unwrap();
        }
        let summary = |budget| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            queue.visit(&mut ledger)?;
            let first = ledger.summary()?;
            queue.visit(&mut ledger)?;
            assert_eq!(ledger.summary()?, first, "same actual fields dedup");
            Ok::<_, AudioStorageError>(first)
        };
        let both = summary(budget).unwrap();
        assert_eq!(
            both.data_bytes - empty.data_bytes,
            actual_backing,
            "omitting either range or quoting len instead of capacity must fail"
        );
        assert_eq!(
            both.data_allocations - empty.data_allocations,
            2,
            "equal track contents are two distinct owned Vec allocations"
        );
        let mut exact = budget;
        exact.max_retained_bytes = both.retained_bytes;
        exact.max_construction_peak_bytes = both.construction_peak_bytes;
        assert_eq!(summary(exact).unwrap(), both);
        let mut short = exact;
        short.max_retained_bytes -= 1;
        assert_eq!(summary(short), Err(AudioStorageError::ReservationExceeded));
        short = exact;
        short.max_construction_peak_bytes -= 1;
        assert_eq!(summary(short), Err(AudioStorageError::ReservationExceeded));
        eprintln!(
            "SUPPORTED actual queued Copy+Cut tracks capacity128 len1 each: additional bytes={actual_backing}, allocations2, same-field dedup, retained/peak exact and each-minus1; metadata component only"
        );
    }

    #[test]
    fn native_request_time_range_foreign_field_refuses() {
        use hex_engine::sampler_host_fields::{
            BodyRef, FamilyRequestFields, FamilyRequestPayload, FamilyRequestRecords, VecRef,
        };
        use hex_project::{Command, Created, Editor, Project, TimeRange};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "range".into(),
            })
            .unwrap()
        else {
            panic!("actual track")
        };
        let mut queue = NativeRequests::default();
        queue
            .prepare(NativeRequests::requested_backing().unwrap())
            .unwrap();
        queue
            .send(&mut Some(Request::CopyTimeRange(
                TimeRange::new(vec![track], 0, 64).unwrap(),
            )))
            .unwrap();
        struct Foreign<'a> {
            queue: &'a NativeRequests,
            range: &'a TimeRange,
        }
        impl FamilyRequestRecords for Foreign<'_> {
            fn body(&self) -> BodyRef<'_> {
                self.queue.body()
            }
            fn slots(&self) -> VecRef<'_> {
                self.queue.slots()
            }
            fn request(
                &self,
                index: usize,
            ) -> Result<FamilyRequestFields<'_>, hex_engine::AudioStorageError> {
                let mut record = self.queue.request(index)?;
                if let Some(FamilyRequestPayload::TimeRange { body, .. }) = record.request {
                    record.request = Some(FamilyRequestPayload::TimeRange {
                        body,
                        range: self.range,
                    });
                }
                Ok(record)
            }
        }
        let foreign = TimeRange::new(vec![track], 0, 64).unwrap();
        let fields = Foreign {
            queue: &queue,
            range: &foreign,
        };
        let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 16,
                root_rows: 4,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            },
        )
        .unwrap();
        assert_eq!(
            ledger.borrow_sampler_family_requests_on_control(&fields),
            Err(hex_engine::AudioStorageError::SourceMismatch)
        );
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
