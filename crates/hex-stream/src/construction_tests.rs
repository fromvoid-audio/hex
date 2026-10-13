use super::*;
use crate::{AllocationCapacity, CompactCopyReservations};
use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
use hex_media::{ResampleKernel, VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project, converted_frame_count};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::AtomicU64,
    time::Instant,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct ReleaseOnDrop(Arc<AtomicBool>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct Wav(PathBuf);
impl Wav {
    fn new(frames: usize) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-construction-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = (frames * 4) as u32;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&48000_u32.to_le_bytes()).unwrap();
        file.write_all(&192000_u32.to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        file.write_all(&vec![0; bytes as usize]).unwrap();
        Self(path)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn limits() -> VerifiedWavReaderLimits {
    VerifiedWavReaderLimits::new(
        WavReaderLimits::new(2 * 1024 * 1024, 500000, 128, 8192),
        1024 * 1024,
    )
}
fn config() -> StreamConfig {
    StreamConfig {
        source_scratch_frames: 8192,
        ..StreamConfig::with_pool_pages(4)
    }
}
fn input(wav: &Wav, count: usize, rate: u32) -> CompactSourceInput {
    let reader = VerifiedWavReader::open(&wav.0, limits()).unwrap();
    let frames = reader.source_frames();
    let mut editor = Editor::new(Project::new(48000).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: wav.0.clone(),
            sample_rate: 48000,
            frames,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let mut definitions = Vec::with_capacity(32);
    for index in 0..count {
        let mut path = PathBuf::with_capacity(4096);
        path.push(&wav.0);
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(20 + index as u64, asset),
            48000,
            rate,
            frames,
            converted_frame_count(frames, 48000, rate).unwrap(),
            if rate == 48000 { 0 } else { 1 },
            *reader.fingerprint(),
        )
        .unwrap();
        definitions.push(SourceDefinition::new(path, metadata, limits()));
    }
    let mut input = CompactSourceInput::new(definitions);
    input
        .try_compact(
            config(),
            CompactCopyReservations {
                max_copy_bytes: 65536,
                max_overlap_bytes: 1024 * 1024,
                other_retained_bytes: 101,
                other_construction_bytes: 202,
            },
        )
        .unwrap();
    input
}
fn make_request(wav: &Wav) -> StreamConstructionRequest {
    StreamConstructionRequest::new(input(wav, 1, 48000), config(), 64 * 1024 * 1024)
}
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    ptr: usize,
    capacity: usize,
    paths: Vec<(usize, usize, PathBuf)>,
    metadata: Vec<StreamedSourceMetadata>,
}
fn identity(originals: &Vec<SourceDefinition>) -> Identity {
    Identity {
        ptr: originals.as_ptr() as usize,
        capacity: originals.capacity(),
        paths: originals
            .iter()
            .map(|d| {
                (
                    d.path.as_os_str().as_encoded_bytes().as_ptr() as usize,
                    d.path.capacity(),
                    d.path.clone(),
                )
            })
            .collect(),
        metadata: originals.iter().map(|d| d.metadata.clone()).collect(),
    }
}
fn recover(request: &mut StreamConstructionRequest) -> RecoveredStreamInput {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(recovered) = request.try_recover().unwrap() {
            return recovered;
        }
        assert!(Instant::now() < deadline, "worker cleanup remains pending");
        thread::sleep(Duration::from_millis(1));
    }
}
fn wait(request: &mut StreamConstructionRequest) -> ConstructionState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = request.poll_timeout(Duration::from_millis(2));
        if state != ConstructionState::Pending {
            return state;
        }
        assert!(Instant::now() < deadline);
    }
}
fn assert_joined(cleanup: ConstructionCleanup) {
    let ConstructionCleanup::Joined(report) = cleanup else {
        panic!("expected normal joined pool: {cleanup:?}");
    };
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed,
        report.page_buffers
    );
}

#[test]
fn successful_finish_is_verified_capacity_checked_and_preserves_originals_and_kernel_sharing() {
    let wav = Wav::new(8192);
    let mut request =
        StreamConstructionRequest::new(input(&wav, 2, 44100), config(), 64 * 1024 * 1024);
    let original = identity(request.originals.as_ref().unwrap());
    assert_eq!(request.state(), ConstructionState::Unstarted);
    request.start().unwrap();
    assert_eq!(wait(&mut request), ConstructionState::CatalogVerified);
    let finished = request.try_finish().unwrap().unwrap();
    assert_eq!(request.state(), ConstructionState::Consumed);
    let (originals, mut runtime, capacity) = finished.into_parts();
    assert_eq!(identity(&originals), original);
    runtime.wait_until_ready(Duration::ZERO).unwrap();
    let kernel = ResampleKernel::new(48000, 44100).unwrap();
    assert_eq!(
        capacity.worker.catalog.kernel_weight_bytes,
        kernel.table_payload_bytes()
    );
    assert_eq!(
        capacity.worker.catalog.kernel_object_bytes,
        kernel.shared_object_bytes()
    );
    assert_eq!(capacity.worker.catalog.membership_slots.capacity, 2);
    assert_eq!(
        capacity.worker.catalog.definition_path_bytes,
        capacity.input.unwrap().compact.path_capacity_bytes
    );
    assert!(capacity.worker.finalized && !capacity.worker.kernel_transient_unobserved);
    assert!(capacity.data_bound_bytes().unwrap() <= capacity.max_construction_data_bytes);
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
    println!(
        "two-source construction capacities: {capacity:?}; bound={:?}",
        capacity.data_bound_bytes()
    );
}

#[test]
fn every_setup_reservation_and_spawn_refusal_preserves_originals_and_reclaims_created_pool() {
    let wav = Wav::new(8192);
    let mut baseline = make_request(&wav);
    baseline.start().unwrap();
    let attempts = baseline.capacity_report().setup.reservation_attempts;
    baseline.cancel().unwrap();
    assert_joined(recover(&mut baseline).cleanup);
    for attempt in 0..attempts {
        let mut request = make_request(&wav);
        let original = identity(request.originals.as_ref().unwrap());
        request.setup_faults.fail_reservation = Some(attempt);
        assert_eq!(
            request.start(),
            Err(ConstructionRefusal::Runtime(
                StreamRuntimeError::AllocationFailed
            ))
        );
        let recovered = recover(&mut request);
        assert_eq!(identity(&recovered.originals), original);
        assert_eq!(recovered.capacity.setup.reservation_attempts, attempt + 1);
        let ConstructionCleanup::NotStarted {
            allocated_pages,
            reclaimed_pages,
        } = recovered.cleanup
        else {
            panic!("no worker");
        };
        assert_eq!(allocated_pages, reclaimed_pages);
        assert_eq!(allocated_pages, attempt.saturating_sub(2).min(4));
    }
    let mut request = make_request(&wav);
    let original = identity(request.originals.as_ref().unwrap());
    request.setup_faults.fail_spawn = true;
    assert_eq!(
        request.start(),
        Err(ConstructionRefusal::Runtime(
            StreamRuntimeError::WorkerSpawnFailed
        ))
    );
    let recovered = recover(&mut request);
    assert_eq!(identity(&recovered.originals), original);
    assert_eq!(
        recovered.cleanup,
        ConstructionCleanup::NotStarted {
            allocated_pages: 4,
            reclaimed_pages: 4
        }
    );
}

#[test]
fn every_worker_reservation_refusal_returns_partial_observations_and_same_original_owners() {
    let wav = Wav::new(8192);
    let mut baseline = make_request(&wav);
    baseline.start().unwrap();
    wait(&mut baseline);
    let attempts = baseline.capacity_report().worker.reservation_attempts;
    baseline.cancel().unwrap();
    assert_joined(recover(&mut baseline).cleanup);
    for attempt in 0..attempts {
        let mut request = make_request(&wav);
        let original = identity(request.originals.as_ref().unwrap());
        request.worker_faults.fail_reservation = Some(attempt);
        request.start().unwrap();
        wait(&mut request);
        assert!(matches!(
            request.failure(),
            Some(ConstructionRefusal::Worker(_))
        ));
        let recovered = recover(&mut request);
        assert_eq!(identity(&recovered.originals), original);
        assert!(recovered.capacity.worker.finalized);
        assert!(recovered.capacity.worker.reservation_attempts > attempt);
        assert_eq!(
            recovered.capacity.worker.catalog.definition_path_bytes,
            recovered
                .capacity
                .input
                .unwrap()
                .compact
                .path_capacity_bytes
        );
        assert_joined(recovered.cleanup);
    }
}

#[test]
fn pending_timeout_and_cancellation_do_not_authorize_finish_or_release_active_owners() {
    let wav = Wav::new(8192);
    let mut request = make_request(&wav);
    let original = identity(request.originals.as_ref().unwrap());
    let hold = Arc::new(AtomicBool::new(false));
    request.worker_faults.hold = Some(hold);
    request.start().unwrap();
    assert_eq!(
        request.poll_timeout(Duration::from_millis(5)),
        ConstructionState::Pending
    );
    assert!(request.try_finish().unwrap().is_none());
    assert_eq!(identity(request.originals.as_ref().unwrap()), original);
    request.cancel().unwrap();
    assert!(request.try_finish().is_err());
    let recovered = recover(&mut request);
    assert_eq!(identity(&recovered.originals), original);
    assert_eq!(recovered.failure, ConstructionRefusal::Cancelled);
    assert_joined(recovered.cleanup);
}

#[test]
fn cancellation_wins_queued_catalog_success() {
    let wav = Wav::new(8192);
    let mut request = make_request(&wav);
    request.start().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !request.capacity_report().worker.finalized {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    // Never consume the result before cancellation; it cannot later escape.
    request.cancel().unwrap();
    assert!(matches!(
        request.try_finish(),
        Err(ConstructionRefusal::Cancelled)
    ));
    assert_joined(recover(&mut request).cleanup);
}

#[test]
fn panic_join_and_accounting_failure_are_explicit_incomplete_recovery() {
    let wav = Wav::new(8192);
    for after_publish in [false, true] {
        let mut request = make_request(&wav);
        let original = identity(request.originals.as_ref().unwrap());
        request.worker_faults.panic_before_catalog = !after_publish;
        request.worker_faults.panic_after_publish = after_publish;
        request.start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !request.worker.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(request.try_finish().is_err());
        let recovered = recover(&mut request);
        assert_eq!(identity(&recovered.originals), original);
        assert!(matches!(
            recovered.cleanup,
            ConstructionCleanup::JoinFailed {
                page_buffers: 4,
                ..
            }
        ));
        assert!(!recovered.capacity.worker.finalized);
        assert!(recovered.capacity.worker.kernel_transient_unobserved);
        assert_eq!(recovered.capacity.finalized_data_bound_bytes(), None);
    }
    let mut request = make_request(&wav);
    request.worker_faults.lose_page = true;
    request.start().unwrap();
    wait(&mut request);
    request.cancel().unwrap();
    let recovered = recover(&mut request);
    let ConstructionCleanup::PoolAccounting(report) = recovered.cleanup else {
        panic!("incomplete inventory required");
    };
    assert!(report.worker_joined);
    assert!(!report.accounting_complete);
    assert_eq!(report.worker_pages_reclaimed, 3);
}

#[test]
fn missing_invalid_and_fingerprint_changed_files_fail_without_escaping_resources() {
    for failure in 0..3 {
        let wav = Wav::new(8192);
        let mut request = make_request(&wav);
        let original = identity(request.originals.as_ref().unwrap());
        match failure {
            0 => fs::remove_file(&wav.0).unwrap(),
            1 => fs::write(&wav.0, b"invalid wav").unwrap(),
            _ => {
                let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
                file.seek(SeekFrom::Start(44)).unwrap();
                file.write_all(&[1, 2, 3, 4]).unwrap();
            }
        }
        request.start().unwrap();
        wait(&mut request);
        assert!(request.try_finish().is_err());
        let recovered = recover(&mut request);
        assert_eq!(identity(&recovered.originals), original);
        if failure == 2 {
            assert_eq!(
                recovered.failure,
                ConstructionRefusal::Worker(StreamFailureKind::SourceChanged)
            );
            assert!(recovered.capacity.worker.catalog.manifest_bytes > 0);
        }
        assert_joined(recovered.cleanup);
    }
}

#[test]
fn changed_already_hashed_content_is_refused_with_partial_manifest_capacity() {
    let wav = Wav::new(34000);
    let mut request = make_request(&wav);
    let original = identity(request.originals.as_ref().unwrap());
    let file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    let file = Arc::new(Mutex::new(file));
    request.worker_faults.on_cancel = Some(Arc::new(move |call| {
        if call == 1 {
            let mut file = file.lock().unwrap();
            file.seek(SeekFrom::Start(48)).unwrap();
            file.write_all(&[1, 2, 3, 4]).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
    }));
    request.start().unwrap();
    wait(&mut request);
    let recovered = recover(&mut request);
    assert_eq!(identity(&recovered.originals), original);
    assert_eq!(
        recovered.failure,
        ConstructionRefusal::Worker(StreamFailureKind::SourceChanged)
    );
    assert!(recovered.capacity.worker.catalog.manifest_bytes >= 96);
    assert_joined(recovered.cleanup);
}

#[test]
fn caller_reservation_refusal_before_spawn_and_after_verification_preserves_ownership() {
    let wav = Wav::new(8192);
    let mut request = StreamConstructionRequest::new(input(&wav, 1, 48000), config(), 1);
    let original = identity(request.originals.as_ref().unwrap());
    assert_eq!(request.start(), Err(ConstructionRefusal::CapacityExceeded));
    assert_eq!(identity(&recover(&mut request).originals), original);
    let mut request = make_request(&wav);
    let original = identity(request.originals.as_ref().unwrap());
    request.start().unwrap();
    wait(&mut request);
    // Explicitly under-reserve the actual finalized observation in this fixture.
    request.capacity.max_construction_data_bytes =
        request.capacity_report().data_bound_bytes().unwrap() - 1;
    assert!(matches!(
        request.try_finish(),
        Err(ConstructionRefusal::CapacityExceeded)
    ));
    let recovered = recover(&mut request);
    assert_eq!(identity(&recovered.originals), original);
    assert_joined(recovered.cleanup);
    let mut report = recovered.capacity;
    report.startup_shared_object_bytes = usize::MAX;
    assert_eq!(report.data_bound_bytes(), None);
    let mut worker = WorkerConstructionObservation::default();
    worker.catalog.source_slots = AllocationCapacity {
        capacity: usize::MAX,
        element_bytes: 2,
    };
    assert_eq!(worker.data_bound_bytes(), None);
}

#[test]
fn active_cleanup_retains_request_owners_until_actual_worker_termination() {
    let wav = Wav::new(8192);
    let mut request = make_request(&wav);
    let original = identity(request.originals.as_ref().unwrap());
    let exit_hold = Arc::new(AtomicBool::new(false));
    let _release_on_panic = ReleaseOnDrop(Arc::clone(&exit_hold));
    request.worker_faults.exit_hold = Some(Arc::clone(&exit_hold));
    request.start().unwrap();
    wait(&mut request);
    request.cancel().unwrap();
    assert_eq!(request.state(), ConstructionState::CancellingCleaning);
    assert!(request.try_recover().unwrap().is_none());
    assert!(request.cleanup().is_none());
    assert_eq!(identity(request.originals.as_ref().unwrap()), original);
    exit_hold.store(true, Ordering::Release);
    let recovered = recover(&mut request);
    assert_eq!(identity(&recovered.originals), original);
    assert_joined(recovered.cleanup);
}

#[test]
fn expected_rate_frames_and_fingerprint_mismatch_are_worker_refusals() {
    let wav = Wav::new(8192);
    for mismatch in 0..3 {
        let (mut originals, _, _) = input(&wav, 1, 48000).into_stage_parts();
        let old = &originals[0].metadata;
        let source_rate = if mismatch == 0 { 44100 } else { 48000 };
        let frames = if mismatch == 1 { 8193 } else { 8192 };
        originals[0].metadata = StreamedSourceMetadata::new(
            old.key(),
            source_rate,
            48000,
            frames,
            converted_frame_count(frames, source_rate, 48000).unwrap(),
            if source_rate == 48000 { 0 } else { 1 },
            if mismatch == 2 {
                [255; 32]
            } else {
                old.fingerprint()
            },
        )
        .unwrap();
        let mut input = CompactSourceInput::new(originals);
        input
            .try_compact(
                config(),
                CompactCopyReservations {
                    max_copy_bytes: 65536,
                    max_overlap_bytes: 1024 * 1024,
                    other_retained_bytes: 0,
                    other_construction_bytes: 0,
                },
            )
            .unwrap();
        let mut request = StreamConstructionRequest::new(input, config(), 64 * 1024 * 1024);
        let original = identity(request.originals.as_ref().unwrap());
        request.start().unwrap();
        wait(&mut request);
        let recovered = recover(&mut request);
        assert_eq!(
            recovered.failure,
            ConstructionRefusal::Worker(StreamFailureKind::SourceChanged)
        );
        assert_eq!(identity(&recovered.originals), original);
        assert_joined(recovered.cleanup);
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn request_construction_and_borrowed_reporting_move_no_owners_or_allocations() {
    use allocation_counter::measure;
    let wav = Wav::new(8192);
    let mut input = Some(input(&wav, 1, 48000));
    let mut request = None;
    let moved = measure(|| {
        request = Some(StreamConstructionRequest::new(
            input.take().unwrap(),
            config(),
            64 * 1024 * 1024,
        ))
    });
    assert_eq!(moved.count_total, 0);
    assert_eq!(moved.count_current, 0);
    assert_eq!(moved.bytes_total, 0);
    assert_eq!(moved.bytes_current, 0);
    let mut request = request.unwrap();
    request.start().unwrap();
    wait(&mut request);
    let references = Arc::strong_count(request.telemetry.as_ref().unwrap());
    let borrowed = measure(|| {
        std::hint::black_box(request.capacity_report());
        std::hint::black_box(request.state());
        std::hint::black_box(request.originals());
    });
    assert_eq!(borrowed.count_total, 0);
    assert_eq!(borrowed.count_current, 0);
    assert_eq!(borrowed.bytes_total, 0);
    assert_eq!(borrowed.bytes_current, 0);
    assert_eq!(
        Arc::strong_count(request.telemetry.as_ref().unwrap()),
        references
    );
    request.cancel().unwrap();
    assert_joined(recover(&mut request).cleanup);
}
