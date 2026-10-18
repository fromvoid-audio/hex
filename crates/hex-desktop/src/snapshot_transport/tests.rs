//! Actual registered adapter and Event moves. No CPAL/backend/app acceptance.
use super::*;
use hex_session::{
    service::{Event, Request, Service, ViewState},
    snapshot_exports::SnapshotExportReservation,
};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub(super) static TABLE_TEST_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn copy_view(source: &ViewState) -> ViewState {
    // Fresh owned view with actual producer child handles; no raw Arc adoption.
    // Legacy preview/parent custody remains unqualified outside this protocol oracle.
    ViewState {
        canonical_audio_source: source.canonical_audio_source.clone(),
        project: source.project.clone(),
        waveforms: source.waveforms.clone(),
        audio_clip_waveforms: source.audio_clip_waveforms.clone(),
        midi_preview: source.midi_preview.clone(),
        gain_preview: source.gain_preview.clone(),
        timeline_frames: source.timeline_frames,
        can_undo: source.can_undo,
        can_redo: source.can_redo,
        revision: source.revision,
        unsaved: source.unsaved,
        document_path: source.document_path.clone(),
    }
}
fn wait_derive(base: &SnapshotReadCapability) -> SnapshotReadCapability {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match base.try_derive() {
            Ok(read) => return read,
            Err(Error::Busy | Error::Reservation) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("derive did not regain its bounded slot: {:?}", result.err()),
        }
    }
}
fn revision(read: &SnapshotReadCapability) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match read.read_with(|snapshot| snapshot.revision()) {
            Ok(revision) => return revision,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("read refused: {result:?}"),
        }
    }
}
fn bind(
    id: RetainedFamilyId<SessionExportFamily>,
    pending: &mut Option<SnapshotReadStamp>,
) -> SnapshotReadCapability {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match bind_registered_read_on_control(id, pending) {
            Ok(read) => return read,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("binding refused: {:?}", result.err()),
        }
    }
}
fn with_family<R>(
    id: RetainedFamilyId<SessionExportFamily>,
    mut read: impl FnMut(&SessionExportFamily) -> R,
) -> R {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |family| read(family)) {
            Ok(result) => return result,
            Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            _ => panic!("registered family refused"),
        }
    }
}
pub(super) fn receive(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            result => panic!("worker Event did not arrive: {:?}", result.err()),
        }
    }
}
fn event(service: &Service, read: SnapshotReadCapability) -> Event {
    service.submit(Request::Undo).unwrap(); // no history: actual unchanged worker response
    let mut event = receive(service);
    assert_eq!(revision(&read), event.state.revision);
    event.snapshot_reads.canonical = Some(read); // before insertion into transport channel
    event
}
fn verify_two_free(base: &SnapshotReadCapability) {
    let first = wait_derive(base);
    let second = wait_derive(base);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match base.try_derive() {
            Err(Error::Reservation) => break,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            _ => panic!("full read reservation must refuse without changing original"),
        }
    }
    assert_eq!(revision(base), 0);
    drop(first);
    drop(second);
}
#[test]
fn actual_registered_event_transport_releases_discarded_queued_preview_and_unpainted_reads() {
    let _serial = TABLE_TEST_GATE.lock().unwrap();
    let failures = failed_release_count();
    hex_audio::reserve_retained_families_on_control(hex_audio::RetainedFamilyReservation {
        family_slots: 2,
        max_table_bytes: 16_384,
    })
    .unwrap(); // existing lifecycle worker, no device/output/native lease
    let service = Service::new(48_000, 8, 1024).unwrap();
    let initial = receive(&service);
    let (family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 3,
        max_bookkeeping_bytes: 16_384,
    })
    .unwrap();
    let snapshot = family
        .insert_on_control(&key, &mut Some(copy_view(&initial.state)))
        .unwrap();
    let mut pending_read = Some(family.reserve_read_on_control(&key, snapshot).unwrap());
    let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
    let id = register_family_on_control(
        &slot,
        &mut Some(family),
        hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
    )
    .unwrap();
    let base = bind(id, &mut pending_read);
    assert!(pending_read.is_none());
    let mut refused_read =
        Some(with_family(id, |family| family.reserve_read_on_control(&key, snapshot)).unwrap());
    let (foreign, foreign_key) =
        SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 1,
            read_records: 1,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap();
    let foreign_slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
    let foreign_id = register_family_on_control(
        &foreign_slot,
        &mut Some(foreign),
        hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match bind_registered_read_on_control(foreign_id, &mut refused_read) {
            Err(Error::Stale) => break,
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            _ => panic!("wrong family must not bind another family's read"),
        }
    }
    assert!(refused_read.is_some());
    release_pending_on_control(id, &mut refused_read).unwrap();
    assert!(refused_read.is_none());
    with_family(foreign_id, |family| family.abandon_on_control(&foreign_key)).unwrap();
    verify_two_free(&base);
    // Real producer abandonment closes future snapshots; old live reads/derive
    // remain valid, independent of channel/UI lifetime.
    with_family(id, |family| family.abandon_on_control(&key)).unwrap();

    drop(event(&service, wait_derive(&base))); // discarded before send
    verify_two_free(&base);
    let (sender, receiver) = mpsc::sync_channel(1);
    assert!(sender.try_send(event(&service, wait_derive(&base))).is_ok());
    let returned = sender.try_send(event(&service, wait_derive(&base)));
    let Err(mpsc::TrySendError::Full(returned)) = returned else {
        panic!("full Event returned")
    };
    drop(returned);
    drop(receiver.recv().unwrap());
    verify_two_free(&base);

    assert!(sender.try_send(event(&service, wait_derive(&base))).is_ok());
    drop(receiver); // queued Event is dropped by the real channel
    let returned = sender.try_send(event(&service, wait_derive(&base)));
    let Err(mpsc::TrySendError::Disconnected(returned)) = returned else {
        panic!("closed Event returned")
    };
    drop(returned);
    verify_two_free(&base);

    let mut preview = event(&service, wait_derive(&base));
    preview.snapshot_reads.device_preview = preview.snapshot_reads.canonical.take();
    preview.snapshot_reads.tuning_preview = Some(wait_derive(&base));
    preview.device_preview = Some(hex_session::service::DevicePreview {
        identity: hex_session::service::DevicePreviewIdentity {
            project_generation: 1,
            key: hex_session::device_gesture::GestureKey {
                generation: 1,
                token: 9,
            },
            sequence: 1,
            canonical_revision: 0,
        },
        state: Arc::clone(&preview.state),
    });
    preview.tuning_preview = Some(hex_session::service::TuningPreview {
        identity: hex_session::service::TuningPreviewIdentity {
            project_generation: 1,
            key: hex_session::tuning_gesture::TuningGestureKey {
                generation: 1,
                token: 10,
            },
            sequence: 1,
            canonical_revision: 0,
        },
        state: Arc::clone(&preview.state),
    });
    drop(preview); // both preview-read slots released even without rendering
    verify_two_free(&base);
    let mut unpainted = event(&service, wait_derive(&base));
    let canvas_read = unpainted.snapshot_reads.canonical.take().unwrap();
    drop(unpainted);
    assert_eq!(revision(&canvas_read), 0);
    drop(canvas_read);
    verify_two_free(&base);
    assert_eq!(failed_release_count(), failures);
    drop(base);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |_| ()) {
            Err(RetainedFamilyError::Stale) => break,
            Ok(()) | Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("actual control retirement did not finish: {result:?}"),
        }
    }
}
