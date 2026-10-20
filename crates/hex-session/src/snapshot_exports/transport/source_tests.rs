//! Routing/claim continuation component, using an actual typed Engine job.
//! No Audio table registration, backend/native admission or Service proof.
use super::*;
use crate::snapshot_exports::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
static JOB: Mutex<Option<hex_engine::RetainedControlJob>> = Mutex::new(None);
static REFUSE_BIND: AtomicBool = AtomicBool::new(false);
fn family<R>(
    route: RetainedControlJobRoute,
    read: impl FnOnce(&SessionExportFamily) -> R,
) -> Result<R, SnapshotTransportError> {
    let job = JOB.lock().unwrap();
    let job = job.as_ref().ok_or(SnapshotTransportError::Stale)?;
    if job.route() != route {
        return Err(SnapshotTransportError::Stale);
    }
    job.inspect_payload::<SessionExportFamily, _>(read)
        .ok_or(SnapshotTransportError::Stale)
}
fn error(error: ExportRecordError) -> SnapshotTransportError {
    match error {
        ExportRecordError::Busy => SnapshotTransportError::Busy,
        ExportRecordError::Reservation => SnapshotTransportError::Reservation,
        _ => SnapshotTransportError::Stale,
    }
}
fn validate(
    route: RetainedControlJobRoute,
    stamp: &SnapshotReadStamp,
) -> Result<(), SnapshotTransportError> {
    if REFUSE_BIND.load(Ordering::Relaxed) {
        return Err(SnapshotTransportError::Busy);
    }
    family(route, |family| {
        family.read_with(stamp, |_| ()).map_err(error)
    })?
}
fn read(
    route: RetainedControlJobRoute,
    stamp: &SnapshotReadStamp,
    continuation: &mut dyn SnapshotReadContinuation,
) -> Result<(), SnapshotTransportError> {
    family(route, |family| {
        family
            .read_with(stamp, |snapshot| continuation.read(snapshot))
            .map_err(error)
    })?
}
fn derive(
    route: RetainedControlJobRoute,
    stamp: &SnapshotReadStamp,
) -> Result<SnapshotReadStamp, SnapshotTransportError> {
    family(route, |family| family.derive_read(stamp).map_err(error))?
}
fn release(
    route: RetainedControlJobRoute,
    stamp: SnapshotReadStamp,
) -> Result<(), SnapshotTransportError> {
    family(route, |family| family.release_read(stamp).map_err(error))?
}
fn report(_: RetainedControlJobRoute, _: SnapshotTransportError) {
    panic!("unexpected release failure");
}
fn follows(
    route: RetainedControlJobRoute,
    current: &SnapshotReadStamp,
    previous: &SnapshotReadStamp,
    generation: u64,
) -> Result<bool, SnapshotTransportError> {
    family(route, |family| {
        family
            .follows_source(current, previous, generation)
            .map_err(error)
    })?
}
static READS: SnapshotReadOperations =
    SnapshotReadOperations::new_unqualified(validate, read, derive, release, report, follows);
fn inspect(
    route: RetainedControlJobRoute,
    read: &mut dyn SourceEventHeadContinuation,
) -> Result<bool, SnapshotTransportError> {
    family(route, |family| {
        family
            .read_source_event_head_on_control(|head, snapshot| read.read(head, snapshot))
            .map(|value| value.is_some())
            .map_err(error)
    })?
}
fn claim(
    route: RetainedControlJobRoute,
    head: SourceEventHead,
    extra: bool,
    destination: &mut Option<SourceEventDelivery>,
) -> Result<(), SnapshotTransportError> {
    family(route, |family| {
        family
            .receive_source_event_on_control(head, extra, destination)
            .map_err(error)
    })?
}
static EVENTS: SnapshotSourceEventOperations =
    SnapshotSourceEventOperations::new_unqualified(inspect, claim);
#[test]
fn binding_busy_retains_exact_claim_and_receiver_drop_acknowledges_all_actual_roles() {
    let (owner, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 4,
        max_bookkeeping_bytes: 16_384,
    })
    .unwrap();
    let session = crate::Session::new(48_000, 1, 1024).unwrap();
    let mut view = Some(crate::service::view_state_owned(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        9,
        1,
        false,
        None,
    ));
    let mut metadata = Some(SourceEventMetadata {
        facts: SourceEventFacts::default(),
        status: Some("retained head".into()),
        error: None,
        audio_batch_failure: None,
        gesture_ack: None,
        tuning_gesture_ack: None,
        tuning_library: None,
    });
    let mut stamp = Some(
        owner
            .try_event_construction_on_control(&key)
            .unwrap()
            .insert_source_on_control(&mut view, &mut metadata)
            .unwrap(),
    );
    owner
        .enqueue_source_event_on_control(&key, &mut stamp, &mut None, &mut None)
        .unwrap();
    let mut second_view = Some(crate::service::view_state_owned(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        10,
        1,
        false,
        None,
    ));
    let mut second_metadata = Some(SourceEventMetadata {
        facts: SourceEventFacts::default(),
        status: Some("second head".into()),
        error: None,
        audio_batch_failure: None,
        gesture_ack: None,
        tuning_gesture_ack: None,
        tuning_library: None,
    });
    let mut second_stamp = Some(
        owner
            .try_event_construction_on_control(&key)
            .unwrap()
            .insert_source_on_control(&mut second_view, &mut second_metadata)
            .unwrap(),
    );
    owner
        .enqueue_source_event_on_control(&key, &mut second_stamp, &mut None, &mut None)
        .unwrap();
    let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(owner),
        usize::MAX,
        |family: &mut SessionExportFamily| {
            family
                .cleanup_one_on_control()
                .map(|progress| match progress {
                    SnapshotFamilyCleanup::Pending => {
                        hex_engine::RetainedControlJobProgress::Pending
                    }
                    SnapshotFamilyCleanup::Disposed => {
                        hex_engine::RetainedControlJobProgress::Disposed
                    }
                })
        },
    )
    .unwrap();
    let route = job.route();
    *JOB.lock().unwrap() = Some(job);
    let mut receiver =
        SnapshotSourceEventReceiver::new_unqualified_on_control(route, &EVENTS, &READS);
    let head = receiver
        .inspect_head_with(|head, snapshot| {
            assert_eq!(
                snapshot.event_metadata().unwrap().status(),
                Some("retained head")
            );
            assert_eq!(snapshot.revision(), 9);
            head
        })
        .unwrap()
        .unwrap();
    REFUSE_BIND.store(true, Ordering::Relaxed);
    assert!(matches!(
        receiver.try_receive(head, true),
        Err(SnapshotTransportError::Busy)
    ));
    assert_ne!(
        family(route, |family| family
            .source_event_head_on_control()
            .unwrap())
        .unwrap(),
        Some(head)
    );
    assert_eq!(
        receiver
            .inspect_head_with(|same, snapshot| (same, snapshot.revision()))
            .unwrap(),
        Some((head, 9))
    );
    assert!(matches!(
        receiver.try_receive(head, false),
        Err(SnapshotTransportError::Stale)
    ));
    REFUSE_BIND.store(false, Ordering::Relaxed);
    let delivery = receiver.try_receive(head, true).unwrap();
    let current = delivery.reads.canonical.as_ref().unwrap();
    let pending = delivery.canonical_pending.as_ref().unwrap();
    assert_eq!(current.source_key(), pending.source_key());
    let derived = current.try_derive().unwrap();
    assert_eq!(current.source_key(), derived.source_key());
    drop(derived);
    assert_eq!(
        delivery
            .reads
            .canonical
            .as_ref()
            .unwrap()
            .read_with(|snapshot| snapshot.revision()),
        Ok(9)
    );
    assert_eq!(
        delivery
            .canonical_pending
            .as_ref()
            .unwrap()
            .read_with(|snapshot| snapshot.revision()),
        Ok(9)
    );
    drop(delivery); // actual registered read-release adapters, no model Drop
    let second_head = receiver
        .inspect_head_with(|head, snapshot| {
            assert_eq!(snapshot.revision(), 10);
            head
        })
        .unwrap()
        .unwrap();
    REFUSE_BIND.store(true, Ordering::Relaxed);
    assert!(matches!(
        receiver.try_receive(second_head, true),
        Err(SnapshotTransportError::Busy)
    ));
    drop(receiver); // ACK BOTH retained raw claim reads, not just bound caps
    REFUSE_BIND.store(false, Ordering::Relaxed);
    // Normal cleanup proves all four actual ACKs were collected.
    let mut slot = JOB.lock().unwrap();
    let mut job = slot.take().unwrap();
    drop(slot);
    job.inspect_payload::<SessionExportFamily, _>(|family| {
        family.abandon_on_control(&key).unwrap()
    })
    .unwrap();
    assert_eq!(
        job.step_on_control().unwrap(),
        hex_engine::RetainedControlJobProgress::Pending
    );
    assert_eq!(
        job.step_on_control().unwrap(),
        hex_engine::RetainedControlJobProgress::Disposed
    );
    drop(job);
}
