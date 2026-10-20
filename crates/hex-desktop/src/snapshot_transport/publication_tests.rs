//! Registered worker route, not production Event/UI or full native publication.
use super::*;
use crate::snapshot_transport::tests as fixtures;
use hex_session::{service::Service, snapshot_exports::SnapshotExportReservation};
use std::time::{Duration, Instant};

#[test]
fn registered_worker_snapshot_refuses_full_read_without_consuming_view_then_reuses_after_ack() {
    let _serial = fixtures::TABLE_TEST_GATE.lock().unwrap();
    fixtures::initialize_test_table();
    let failures = failed_release_count();
    let service = Service::new(48_000, 8, 1024).unwrap();
    let original = fixtures::receive(&service);
    let (family, producer) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 1,
        max_bookkeeping_bytes: 16_384,
    })
    .unwrap();
    let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
    let id = register_family_on_control(
        &slot,
        &mut Some(family),
        hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
    )
    .unwrap();
    let mut pending = Some(fixtures::copy_view(&original.state));
    let mut read = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    let first = loop {
        match publish_pending_snapshot_on_control(id, &producer, &mut pending, &mut read) {
            Ok(capability) => break capability,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("registered worker publication failed: {:?}", result.err()),
        }
    };
    assert!(pending.is_none() && read.is_none());
    assert_eq!(fixtures::revision(&first), original.state.revision);
    let mut replacement = fixtures::copy_view(&original.state);
    replacement.revision += 1;
    pending = Some(replacement);
    let field = std::ptr::from_ref(&pending.as_ref().unwrap().project);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match publish_pending_snapshot_on_control(id, &producer, &mut pending, &mut read) {
            Err(Error::Reservation) => break,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("full real read slot must refuse: {:?}", result.err()),
        }
    }
    assert_eq!(
        std::ptr::from_ref(&pending.as_ref().unwrap().project),
        field
    );
    assert!(read.is_none());
    assert_eq!(fixtures::revision(&first), original.state.revision);
    drop(first); // durable ACK only; actual persistent service reclaims later
    let deadline = Instant::now() + Duration::from_secs(5);
    let replacement = loop {
        match publish_pending_snapshot_on_control(id, &producer, &mut pending, &mut read) {
            Ok(capability) => break capability,
            Err(Error::Busy | Error::Reservation) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("real ACK must permit bounded reuse: {:?}", result.err()),
        }
    };
    assert!(pending.is_none() && read.is_none());
    assert_eq!(
        fixtures::revision(&replacement),
        original.state.revision + 1
    );
    drop(replacement);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |family| family.abandon_on_control(&producer)) {
            Ok(Ok(())) => break,
            Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("producer revocation failed: {result:?}"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |_| ()) {
            Err(RetainedFamilyError::Stale) => break,
            Ok(()) | Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("registered family did not retire: {result:?}"),
        }
    }
    assert_eq!(failed_release_count(), failures);
}
