use super::*;
use hex_engine::{ScalarSourceKey, StreamResourceExtent, StreamResourceKey};
use hex_project::{AutomationValue, Command, Created, Editor, LoopRegion, Project};
use hex_storage::automation_draft::{
    AppliedEvent, CaptureClock, CompletedAutomationTake, DraftClock, DraftProvenance,
    DraftReadLimits, DraftTarget, DraftTerminal, DraftWriter,
};
use std::{fs, path::PathBuf, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct DraftFile(PathBuf);
impl Drop for DraftFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn definition(looped: bool, count: u64) -> (DraftFile, ScalarSourceDefinition) {
    let path = std::env::temp_dir().join(format!(
        "hex-scalar-stream-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut editor = Editor::new(Project::new(48000).unwrap(), 4).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "scalar".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let provenance = DraftProvenance {
        sample_rate: 48000,
        output_identity: 1,
        project_generation: 7,
        editor_revision: 0,
        audio_revision: 0,
        transport_epoch: 1,
        gesture_serial: 1,
    };
    let mut writer = DraftWriter::create_with_clock(
        &path,
        editor.project(),
        DraftTarget::TrackGain { track },
        provenance,
        CaptureClock {
            sample_rate: 48000,
            loop_region: looped.then(|| LoopRegion::new(0, 24, true).unwrap()),
        },
    )
    .unwrap();
    let events = if looped {
        vec![
            AppliedEvent {
                sequence: 1,
                performance_frame: 100,
                content_frame: 20,
                loop_pass: 0,
                value: AutomationValue::new(0.0).unwrap(),
            },
            AppliedEvent {
                sequence: 2,
                performance_frame: 104,
                content_frame: 0,
                loop_pass: 1,
                value: AutomationValue::new(0.75).unwrap(),
            },
        ]
    } else {
        (0..count)
            .map(|index| AppliedEvent {
                sequence: index + 1,
                performance_frame: index,
                content_frame: index,
                loop_pass: 0,
                value: AutomationValue::new(if index % 2 == 0 { 0.0 } else { 0.75 }).unwrap(),
            })
            .collect()
    };
    for batch in events.chunks(64) {
        writer.append(batch).unwrap();
    }
    writer
        .finish(DraftTerminal::Finished(if looped {
            DraftClock {
                performance_frame: 108,
                content_frame: 4,
                loop_pass: 1,
            }
        } else {
            DraftClock {
                performance_frame: count,
                content_frame: count,
                loop_pass: 0,
            }
        }))
        .unwrap();
    let source = CompletedAutomationTake::open_on_control(
        &path,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap()
    .into_prepared_on_control()
    .unwrap();
    let projection = Arc::new(source.captured_tempo_map().clone());
    let definition =
        ScalarSourceDefinition::new(ScalarSourceKey::new(7, 1, 1).unwrap(), source, projection)
            .unwrap();
    (DraftFile(path), definition)
}
fn audio_definition() -> (crate::worker::tests::Fixture, SourceDefinition) {
    let wav = crate::worker::tests::Fixture::pcm16_frames(16001);
    let limits = hex_media::VerifiedWavReaderLimits::new(
        hex_media::WavReaderLimits::new(2 * 1024 * 1024, 16001, 128, 32768),
        1024 * 1024,
    );
    let reader = hex_media::VerifiedWavReader::open(&wav.0, limits).unwrap();
    let mut editor = Editor::new(Project::new(48000).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "mixed.wav".into(),
            sample_rate: 48000,
            frames: 16001,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let metadata = hex_engine::StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset),
        48000,
        48000,
        16001,
        16001,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let definition = SourceDefinition::new(wav.0.clone(), metadata, limits);
    (wav, definition)
}
fn await_scalar(pages: &mut StreamCallbackPages, key: ScalarSourceKey) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        pages.pump_ready(8);
        if pages.scalar_page(key, 0).is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "scalar page timed out"
        );
        std::thread::yield_now();
    }
}
#[test]
fn scalar_installed_zero_and_gap_are_distinct_from_missing_and_catalog_facts_are_exact() {
    let (_file, definition) = definition(true, 0);
    let metadata = definition.metadata().clone();
    let key = metadata.key();
    let mut runtime = StreamRuntime::spawn_with_scalar_sources(
        StreamConfig::with_pool_pages(3),
        vec![],
        vec![definition],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, mut pages) = runtime.split().ok().unwrap();
    let lease = control.catalog_lease().unwrap();
    assert!(lease.covers_scalar_source(&metadata));
    let mut wrong_projection = metadata.projection_identity();
    wrong_projection.fingerprint[0] ^= 1;
    let wrong = hex_engine::ScalarSourceMetadata::new(
        key,
        metadata.project_rate(),
        metadata.start_frame(),
        metadata.end_frame(),
        wrong_projection,
        metadata.file_identity(),
    )
    .unwrap();
    assert!(!lease.covers_scalar_source(&wrong));
    assert_eq!(lease.scalar_shared_data().len(), 1);
    assert!(lease.footprint().sources == 1);
    assert!(pages.scalar_page(key, 0).is_none());
    pages.request_scalar_page(key, 0);
    control.wake_worker();
    await_scalar(&mut pages, key);
    let page = pages.scalar_page(key, 0).unwrap();
    assert_eq!(page.values.len(), 24);
    assert_eq!(&page.values[..4], &[0.75; 4]);
    for offset in 4..20 {
        assert_eq!((page.coverage[offset / 64] >> (offset % 64)) & 1, 0);
    }
    for offset in 20..24 {
        assert_eq!(page.values[offset], 0.0);
        assert_eq!((page.coverage[0] >> offset) & 1, 1);
    }
    assert!(
        pages
            .scalar_page(ScalarSourceKey::new(7, 1, 2).unwrap(), 0)
            .is_none()
    );
    assert!(
        pages
            .page(
                StreamSourceKey::new(7, {
                    let mut e = Editor::new(Project::new(48000).unwrap(), 4).unwrap();
                    let Created::Asset(a) = e
                        .execute(Command::AddAsset {
                            path: "unrelated.wav".into(),
                            sample_rate: 48000,
                            frames: 24,
                        })
                        .unwrap()
                    else {
                        panic!("asset")
                    };
                    a
                }),
                0
            )
            .is_none()
    );
    let data = lease.catalog.data;
    assert_eq!(data.scalar_source_handles, 1);
    assert_eq!(data.scalar_cursor_handle_bound, 2);
    drop(lease);
    let report = control.shutdown(pages).unwrap();
    assert!(report.accounting_complete);
    assert_eq!(report.page_bytes, 3 * (4096 * 8 + 4096 * 4 + 64 * 8));
}
#[test]
fn scalar_job_is_resumable_never_publishes_partial_page_and_damage_refuses() {
    let (file, mut definition) = definition(false, 2048);
    let key = crate::ResourcePageKey {
        source: definition.metadata().key().into(),
        page_index: 0,
    };
    let page = PageBuffer::new_with_capacity(true).unwrap().0;
    let mut job = crate::worker::scalar::ScalarJob::begin(0, &mut definition, key, page)
        .ok()
        .unwrap();
    assert!(!job.step(&mut definition).unwrap());
    assert_eq!(job.page.key, None);
    let mut steps = 1;
    while !job.step(&mut definition).unwrap() {
        steps += 1;
        assert!(steps < 100);
    }
    assert!(steps > 1);
    assert_eq!(job.page.key, Some(key));
    for index in 0..2048 {
        assert_eq!(
            job.page.scalar_values[index],
            if index % 2 == 0 { 0.0 } else { 0.75 }
        );
    }
    drop(job);
    let page = PageBuffer::new_with_capacity(true).unwrap().0;
    let mut job = crate::worker::scalar::ScalarJob::begin(0, &mut definition, key, page)
        .ok()
        .unwrap();
    assert!(!job.step(&mut definition).unwrap());
    let bytes = fs::metadata(&file.0).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&file.0)
        .unwrap()
        .set_len(bytes / 2)
        .unwrap();
    let mut failed = false;
    for _ in 0..100 {
        match job.step(&mut definition) {
            Err(StreamFailureKind::SourceChanged) => {
                failed = true;
                break;
            }
            Ok(false) => {}
            other => panic!("expected damaged pending source refusal, got {other:?}"),
        }
    }
    assert!(failed);
    assert_eq!(job.page.key, None);
}
#[test]
fn mixed_prebound_domain_uses_one_union_capacity_and_same_pair_disposal() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (_file, definition) = definition(true, 0);
    let metadata = definition.metadata().clone();
    let config = StreamConfig::with_pool_pages(3);
    let defs = vec![definition];
    let (_wav, audio) = audio_definition();
    let audio_key = audio.key;
    let audio_defs = vec![audio];
    let reservation = Some(StreamReadinessReservation {
        extents_per_role: 2,
    });
    let bound = PendingStreamDomain::preparation_bound_with_scalar_sources(
        config,
        &audio_defs,
        audio_defs.capacity(),
        &defs,
        defs.capacity(),
        reservation,
    )
    .unwrap();
    let mut pending = PendingStreamDomain::try_start_with_scalar_sources_on_control(
        config,
        audio_defs,
        defs,
        reservation,
        StreamPreparationAllowance {
            max_retained_data_bytes: bound.retained_data_bytes,
            max_construction_peak_data_bytes: bound.construction_peak_data_bytes,
            max_shared_handle_bound: bound.shared_handle_bound,
        },
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !pending.poll_catalog_on_control().unwrap() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let mut domain = pending.try_seal_on_control().unwrap().unwrap();
    assert!(domain.covers_scalar_source(&metadata));
    let mut loan = domain.loan_pages_on_control().unwrap();
    let stamp = ReadinessStamp {
        request_epoch: 1,
        artifact: ReadinessArtifact::Ordinary { publication: 1 },
        project_generation: 7,
        editor_revision: 0,
        device_epoch: 1,
        device_rate: 48000,
        predecessor: None,
        admitted_command_prefix: 0,
        window: hex_engine::StreamChunkWindow {
            performance_frame: 0,
            arrangement_frame: 0,
            route_generation: 0,
            frames: 24,
        },
    };
    let extents = [StreamResourceExtent {
        source: StreamResourceKey::Scalar(metadata.key()),
        page_index: 0,
        start: 0,
        end: 24,
    }];
    let active_stamp = stamp;
    let audio_extents = [StreamResourceExtent {
        source: audio_key.into(),
        page_index: 0,
        start: 0,
        end: 24,
    }];
    loan.submit_resources(ProtectionRole::Active, active_stamp, &audio_extents)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while loan
        .service(ProtectionRole::Active, active_stamp, 2, 2)
        .unwrap()
        == ReadinessProgress::Pending
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(loan.acknowledge_resources(ProtectionRole::Active, active_stamp, &audio_extents));
    let stamp = ReadinessStamp {
        request_epoch: 2,
        ..stamp
    };
    loan.submit_resources(ProtectionRole::Candidate, stamp, &extents)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while loan
        .service(ProtectionRole::Candidate, stamp, 2, 2)
        .unwrap()
        == ReadinessProgress::Pending
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(loan.acknowledge_resources(ProtectionRole::Candidate, stamp, &extents));
    assert!(
        loan.protected_resource_pages(ProtectionRole::Candidate, stamp, &extents)
            .unwrap()
            .scalar_page(metadata.key(), 0)
            .is_some()
    );
    assert_eq!(
        loan.submit_resources(
            ProtectionRole::Transition,
            ReadinessStamp {
                request_epoch: 3,
                ..stamp
            },
            &[StreamResourceExtent {
                source: audio_key.into(),
                page_index: 1,
                start: 0,
                end: 8
            }]
        ),
        Err(ReadinessRefusal::Capacity)
    );
    assert!(loan.is_resource_protected(ProtectionRole::Active, active_stamp, &audio_extents));
    assert!(loan.promote_resources_to_active(ProtectionRole::Candidate, stamp, &extents));
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let mut report = None;
    for _ in 0..10 {
        if let Some(done) = domain.advance_disposal_on_control().unwrap() {
            report = Some(done);
            break;
        }
    }
    assert!(report.unwrap().accounting_complete);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn mixed_scalar_pcm_request_borrow_and_cache_turnover_have_zero_callback_allocations() {
    let (_file, definition) = definition(true, 0);
    let scalar_key = definition.metadata().key();
    let (_wav, audio) = audio_definition();
    let audio_key = audio.key;
    let mut runtime = StreamRuntime::spawn_with_scalar_sources(
        StreamConfig::with_pool_pages(3),
        vec![audio],
        vec![definition],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, mut pages) = runtime.split().ok().unwrap();
    let observed = allocation_counter::measure(|| {
        pages.request_scalar_page(scalar_key, 0);
        pages.request_page(audio_key, 0);
    });
    assert_zero(observed);
    control.wake_worker();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        pages.pump_ready(3);
        if pages.scalar_page(scalar_key, 0).is_some() && pages.page(audio_key, 0).is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_zero(allocation_counter::measure(|| {
        for _ in 0..100 {
            let scalar = pages.scalar_page(scalar_key, 0).unwrap();
            assert_eq!(scalar.values[20], 0.0);
            assert_eq!((scalar.coverage[0] >> 20) & 1, 1);
            assert_eq!(pages.page(audio_key, 0).unwrap().frames.len(), 4096);
        }
        pages.request_page(audio_key, 1);
    }));
    control.wake_worker();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        assert_zero(allocation_counter::measure(|| {
            pages.pump_ready(3);
        }));
        if pages.page(audio_key, 1).is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(control.shutdown(pages).unwrap().accounting_complete);
}
#[cfg(feature = "allocation-probe")]
fn assert_zero(observed: allocation_counter::AllocationInfo) {
    assert_eq!(observed.count_total, 0);
    assert_eq!(observed.bytes_total, 0);
    assert_eq!(observed.count_current, 0);
    assert_eq!(observed.bytes_current, 0);
}
