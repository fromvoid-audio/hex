use super::*;
use hex_project::{AssetSnapshot, ClipSnapshot, TrackContentSnapshot, TrackSnapshot};
use std::{
    collections::BTreeMap,
    io::Write,
    thread,
    time::{Duration, Instant},
};

static FILE_ID: AtomicU64 = AtomicU64::new(1);
struct FixtureFile(std::path::PathBuf);
impl Drop for FixtureFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn request(id: u64) -> (StreamPublicationRequest, FixtureFile) {
    let (request, file, _) = request_internal(id, None);
    (request, file)
}
fn request_internal(
    id: u64,
    bridge_bytes: Option<usize>,
) -> (
    StreamPublicationRequest,
    FixtureFile,
    Option<hex_engine::live_input::TypingTarget>,
) {
    let path = std::env::temp_dir().join(format!(
        "hex-stream-bridge-{}-{}.wav",
        std::process::id(),
        FILE_ID.fetch_add(1, Relaxed)
    ));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + 8192 * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    for value in [1_u16, 2] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [48_000_u32, 192_000] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [4_u16, 16] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(8192_u32 * 4).to_le_bytes());
    for _ in 0..8192 {
        bytes.extend_from_slice(&1000_i16.to_le_bytes());
        bytes.extend_from_slice(&(-1000_i16).to_le_bytes());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
    snapshot.next_id = 4;
    snapshot.assets.push(AssetSnapshot {
        id: 1,
        path: path.clone(),
        source_sample_rate: 48_000,
        source_frames: 8192,
        playback_frames: 8192,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    snapshot.tracks.push(TrackSnapshot {
        gain_envelope: Vec::new(),
        id: 2,
        name: "fixture".into(),
        gain: 1.0,
        muted: false,
        balance: 0.0,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
            id: 3,
            asset: 1,
            start: 0,
            duration: 8192,
            source_offset: 0,
            source_length: 8192,
            source_phase: 0,
            looping: false,
            fade: hex_project::ClipFade::neutral(8192).unwrap(),
        }]),
        sends: Vec::new(),
    });
    let mut project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let target = if bridge_bytes.is_some() {
        let mut editor = hex_project::Editor::new(project, 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "live".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        project = hex_project::Project::from_snapshot(editor.project().snapshot()).unwrap();
        Some(hex_engine::live_input::TypingTarget {
            project_generation: id,
            project_revision: 1,
            track,
            device,
        })
    } else {
        None
    };
    let asset = project.assets().next().unwrap().id();
    // SHA256 of this exact original fixture, independently computed; the real
    // worker must verify it, pin its descriptor, decode and install its pages.
    let fingerprint = [
        69, 252, 130, 129, 235, 215, 253, 236, 187, 94, 66, 115, 169, 25, 66, 181, 67, 35, 192, 44,
        29, 231, 253, 3, 67, 75, 14, 8, 160, 25, 1, 40,
    ];
    let metadata = hex_engine::StreamedSourceMetadata::new(
        hex_engine::StreamSourceKey::new(id, asset),
        48_000,
        48_000,
        8192,
        8192,
        hex_project::IDENTITY_CONVERSION,
        fingerprint,
    )
    .unwrap();
    let limits = hex_stream::VerifiedWavReaderLimits::new(
        hex_stream::WavReaderLimits::new(1024 * 1024, 100_000, 128, 4096),
        1024,
    );
    let definition = hex_stream::SourceDefinition::new(path.clone(), metadata.clone(), limits);
    let mut runtime = hex_stream::StreamRuntime::new(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let catalog = control.catalog_lease().unwrap();
    let sources = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
    )]);
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &sources,
            id,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    if let Some(target) = target {
        playback
            .prepare_typing_input_with_bridge(
                hex_engine::live_input::PreparedTypingArm::prepare(&project, target).unwrap(),
                1,
                bridge_bytes.unwrap(),
            )
            .unwrap();
    }
    let verified = hex_stream::VerifiedStreamRevision::new(revision, catalog).unwrap();
    let endpoint = hex_stream::PageReadiness::new(
        hex_stream::VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap(),
        3,
    )
    .unwrap();
    let input = hex_stream::MediaDomainInput {
        id: hex_stream::DomainId::new(id).unwrap(),
        endpoint,
        retained_leases: {
            let mut leases = Vec::with_capacity(7);
            leases.push(control.catalog_lease().unwrap());
            leases
        },
        control,
    };
    (
        StreamPublicationRequest {
            playback,
            verified,
            source: StreamPublicationSource::New(Box::new(input)),
            policy: PublicationPolicy::InstallFreshRuntime,
            context: StreamingContext {
                project_generation: id,
                editor_revision: 1,
                device_epoch: 1,
                timeline_epoch: 1,
            },
        },
        FixtureFile(path),
        target,
    )
}

fn shutdown(control: &mut Control, callback: &mut Callback, root: &mut StreamBridgeControl) {
    control.request_shutdown();
    for _ in 0..8 {
        callback.begin_block();
        let _reports = root.reclaim(control).unwrap();
    }
    root.return_stopped_batch(callback).unwrap();
    let _reports = root.reclaim(control).unwrap();
    assert_eq!(
        root.cleanup_phase(),
        hex_stream::MediaCleanupPhase::Complete
    );
}

#[test]
fn cold_exact_demand_buffers_without_clock_advance_then_real_installed_pages_render() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (request, _file) = request(1);
    control.publish_streamed(&mut root, request).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[9.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 16]);
    assert!(control.status.buffering.load(Relaxed));
    assert_eq!(control.status.performance_frame.load(Relaxed), 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while control.status.performance_frame.load(Relaxed) == 0 {
        assert!(Instant::now() < deadline);
        thread::yield_now();
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    assert_eq!(output, [[1000.0 / 32768.0, -1000.0 / 32768.0]; 16]);
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_typing_output(true);
    });
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.bytes_current, 0);
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn wrong_bridge_and_third_actual_catalog_refuse_with_original_lease_storage() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (mut other_control, mut other_callback, mut other_root) =
        channel_with_streaming(48_000, 1, 3).unwrap();
    let (first, _one) = request(11);
    let error = control
        .publish_streamed(&mut other_root, first)
        .unwrap_err();
    assert_eq!(error.reason, "stream bridge identity mismatch");
    control.publish_streamed(&mut root, *error.request).unwrap();
    callback.begin_block();
    let (second, _two) = request(12);
    control.publish_streamed(&mut root, second).unwrap();
    callback.begin_block();
    let (third, _three) = request(13);
    let ptr = match &third.source {
        StreamPublicationSource::New(input) => input.retained_leases.as_ptr(),
        _ => unreachable!(),
    };
    let refused = control.publish_streamed(&mut root, third).unwrap_err();
    assert_eq!(
        refused.reason,
        "stream domain slots are occupied; consume cleanup reports and retry"
    );
    assert!(refused.retryable);
    assert!(
        matches!(&refused.request.source,StreamPublicationSource::New(input) if input.retained_leases.as_ptr()==ptr)
    );
    for _ in 0..4 {
        let _reports = root.reclaim(&mut control).unwrap();
        callback.begin_block();
    }
    control
        .publish_streamed(&mut root, *refused.request)
        .unwrap();
    callback.begin_block();
    shutdown(&mut control, &mut callback, &mut root);
    shutdown(&mut other_control, &mut other_callback, &mut other_root);
}

#[test]
fn buffering_preserves_pending_typing_without_ack_and_priority_stop_still_wins() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (request, _file, target) =
        request_internal(21, Some(root.input_bridge_storage_bytes().unwrap()));
    control.publish_streamed(&mut root, request).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    let target = target.unwrap();
    let action = hex_engine::typing_protocol::Action::On {
        owner: target.owner().unwrap(),
        token: hex_engine::typing_protocol::Token {
            epoch: 1,
            serial: 1,
        },
        key: 9,
        pitch: 69,
        velocity: 100,
    };
    let ticket = control.typing_with_ticket(action).unwrap();
    callback.begin_block();
    let mut pcm = [[0.0; 2]; 16];
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_eq!(control.status.performance_frame.load(Relaxed), 0);
    assert!(control.status.typing.snapshot().unwrap().applications[9].is_none());
    control.stop();
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    assert!(!control.status.playing.load(Relaxed));
    assert_eq!(control.status.performance_frame.load(Relaxed), 0);
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn cancelled_queued_source_returns_uninstalled_pair_without_waiting_for_pages() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (mut request, _file) = request(22);
    request.policy = PublicationPolicy::InstallFreshRuntimeIfUnchanged(
        control.capture_fresh_runtime_fence().unwrap(),
    );
    let revision = control.publish_streamed(&mut root, request).unwrap();
    assert_eq!(
        control.cancel_fresh_runtime_publication(revision),
        FreshRuntimeCancellationResult::Cancelled
    );
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Relaxed), 0);
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    assert!(
        root.reclaim(&mut control)
            .unwrap()
            .iter()
            .flatten()
            .all(|(_, report)| report.successful_report().is_some())
    );
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn native_root_harness_keeps_terminal_and_actual_conservation_after_callback_disposal() {
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (request, _file) = request(23);
    root.publish(&mut control, request).unwrap();
    callback.begin_block();
    assert!(root.input_bridge_storage_bytes().unwrap() > input_bridge_storage_bytes().unwrap());
    let (mut guard, errors, mut terminal) =
        crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
    let address = guard.owner_mut() as *const _ as usize;
    drop(control);
    assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
    thread::spawn(move || drop(guard)).join().unwrap();
    assert!(terminal.try_take().is_none());
    assert_eq!(root.tick(false), hex_stream::MediaCleanupPhase::Pending);
    thread::spawn(move || drop(errors)).join().unwrap();
    let (owner, errors) = terminal.try_take().unwrap();
    assert_eq!(owner.as_ref() as *const _ as usize, address);
    drop((owner, errors)); // Harness control disposal; no native pause claim.
    for _ in 0..4 {
        let _phase = root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[test]
fn real_verified_page_mutation_faults_without_advancing_cold_chunk() {
    use std::io::{Seek, SeekFrom};
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (request, file) = request(24);
    control.publish_streamed(&mut root, request).unwrap();
    callback.begin_block();
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&file.0)
        .unwrap();
    writer.seek(SeekFrom::Start(44)).unwrap();
    writer.write_all(&[0, 0, 0, 0]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = callback.render(&mut [[0.0; 2]; 16]);
        if let Err(error) = result {
            assert_eq!(
                error.0,
                "stream source changed during verified page delivery"
            );
            callback.report_render_fault(Some(&error), 0);
            break;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
        callback.begin_block();
    }
    assert_eq!(control.status.performance_frame.load(Relaxed), 0);
    assert!(!control.status.playing.load(Relaxed));
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn same_actual_catalog_reuses_endpoint_but_project_context_change_refuses() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (first, _file) = request(31);
    control.publish_streamed(&mut root, first).unwrap();
    callback.begin_block();
    let domain = hex_stream::DomainId::new(31).unwrap();
    let catalog = root.reaper.catalog_lease(domain).unwrap();
    let revision = Arc::clone(
        callback
            .active
            .as_ref()
            .unwrap()
            .playback
            .prepared_revision()
            .unwrap(),
    );
    let next = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    let request = StreamPublicationRequest {
        playback: next,
        verified: hex_stream::VerifiedStreamRevision::new(revision, catalog).unwrap(),
        source: StreamPublicationSource::Reuse(domain),
        policy: PublicationPolicy::InheritRuntime,
        context: StreamingContext {
            project_generation: 32,
            editor_revision: 2,
            device_epoch: 1,
            timeline_epoch: 1,
        },
    };
    let mut refused = *control
        .publish_streamed(&mut root, request)
        .unwrap_err()
        .request;
    refused.context.project_generation = 31;
    control.publish_streamed(&mut root, refused).unwrap();
    // Existing return pressure must keep the predecessor live until space is
    // available; no opaque retired owner may be silently dropped on callback.
    for revision in [90, 91] {
        callback
            .retired
            .push(GraphReturn::Live(Publication {
                stream_source: None,
                revision,
                control_context_generation: 0,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
                playback: Box::new(Playback::new(
                    hex_engine::Project::new(48_000, Vec::new()).unwrap(),
                ))
                .into(),
            }))
            .ok()
            .unwrap();
    }
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Relaxed), 1);
    assert!(callback.active.is_some());
    let _reports = root.reclaim(&mut control).unwrap();
    let allocations = allocation_counter::measure(|| callback.begin_block());
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.bytes_current, 0);
    let returned = control.retired.as_mut().unwrap().pop().ok().unwrap();
    assert!(matches!(returned, GraphReturn::Retired(_)));
    drop(returned); // Actual opaque predecessor disposal belongs to control.
    assert_eq!(control.status.applied_revision.load(Relaxed), 2);
    let mut domains = 0;
    root.reaper.visit_domains(|_, _| domains += 1);
    assert_eq!(domains, 1);
    let _reports = root.reclaim(&mut control).unwrap();
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn later_missing_chunk_keeps_successful_prefix_pcm_and_advance() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    let (request, _file) = request(32);
    control.publish_streamed(&mut root, request).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let deadline = Instant::now() + Duration::from_secs(2);
    while control.status.performance_frame.load(Relaxed) == 0 {
        callback.render(&mut [[0.0; 2]; 16]).unwrap();
        assert!(Instant::now() < deadline);
        thread::yield_now();
        callback.begin_block();
    }
    control.command(PlaybackCommand::Seek(3840)).unwrap();
    callback.begin_block();
    let before = control.status.performance_frame.load(Relaxed);
    let mut output = [[9.0; 2]; 512];
    callback.render(&mut output).unwrap();
    assert_eq!(
        &output[..256],
        &[[1000.0 / 32768.0, -1000.0 / 32768.0]; 256]
    );
    assert_eq!(&output[256..], &[[0.0; 2]; 256]);
    assert_eq!(control.status.performance_frame.load(Relaxed), before + 256);
    assert!(control.status.buffering.load(Relaxed));
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn extra_graph_catalog_lease_keeps_terminal_cleanup_pending_without_invented_join() {
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (request, _file) = request(33);
    root.publish(&mut control, request).unwrap();
    callback.begin_block();
    let held = root
        .catalog_lease(hex_stream::DomainId::new(33).unwrap())
        .unwrap();
    drop(control);
    drop(callback);
    assert_eq!(root.tick(true), hex_stream::MediaCleanupPhase::Pending);
    assert!(root.pool_reports().unwrap().iter().all(Option::is_none));
    drop(held);
    for _ in 0..4 {
        let _phase = root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[test]
fn bridge_reports_actual_reservations_and_checked_overflow_without_process_claim() {
    let (mut control, mut callback, mut root) = channel_with_streaming(48_000, 1, 3).unwrap();
    assert!(root.input_bridge_storage_bytes().unwrap() > input_bridge_storage_bytes().unwrap());
    eprintln!(
        "stream bridge exclusive_data_bytes={:?}; escrow={:?}; shared_domain_data_bytes={:?}",
        root.input_bridge_storage_bytes(),
        root.allocation_footprint(),
        root.allocation_footprint().domain_data_bytes
    );
    let saved = root.demand_capacities[0];
    root.demand_capacities[0].capacity = usize::MAX;
    assert_eq!(root.input_bridge_storage_bytes(), None);
    root.demand_capacities[0] = saved;
    shutdown(&mut control, &mut callback, &mut root);
}

#[test]
fn retiring_unwind_keeps_candidate_and_predecessor_in_persistent_publications() {
    for after_commit in [false, true] {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (first, _file) = request(if after_commit { 72 } else { 71 });
        let id = first.context.project_generation;
        root.publish(&mut control, first).unwrap();
        callback.begin_block();
        let old_address = callback
            .active
            .as_ref()
            .unwrap()
            .playback
            .live
            .as_ref()
            .unwrap()
            .as_ref() as *const Playback as usize;
        let domain = hex_stream::DomainId::new(id).unwrap();
        let revision = Arc::clone(
            callback
                .active
                .as_ref()
                .unwrap()
                .playback
                .prepared_revision()
                .unwrap(),
        );
        let weak = Arc::downgrade(&revision);
        let catalog = root.catalog_lease(domain).unwrap();
        root.publish(
            &mut control,
            StreamPublicationRequest {
                playback: Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap(),
                verified: hex_stream::VerifiedStreamRevision::new(revision, catalog).unwrap(),
                source: StreamPublicationSource::Reuse(domain),
                policy: PublicationPolicy::InheritRuntime,
                context: StreamingContext {
                    project_generation: id,
                    editor_revision: 2,
                    device_epoch: 1,
                    timeline_epoch: 1,
                },
            },
        )
        .unwrap();
        let before = weak.strong_count();
        callback.test_retiring_panic_after_commit = Some(after_commit);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback.begin_block()));
        assert!(result.is_err());
        assert_eq!(weak.strong_count(), before);
        let old = callback.active.as_ref().unwrap();
        assert_eq!(old.revision, 1);
        assert_eq!(old.control_context_generation, id);
        assert_eq!(old.stream_source.as_ref().unwrap().domain, domain);
        assert_eq!(old.playback.retired.is_some(), after_commit);
        assert_eq!(old.playback.live.is_none(), after_commit);
        if !after_commit {
            assert_eq!(
                old.playback.live.as_ref().unwrap().as_ref() as *const Playback as usize,
                old_address
            );
        }
        assert_eq!(callback.deferred.as_ref().unwrap().revision, 2);
        assert!(callback.deferred.as_ref().unwrap().playback.live.is_some());
        assert_eq!(control.status.applied_revision.load(Relaxed), 1);
        // Quarantined runtime must not be rendered/retried after an unwind.
        // Confirmed control-side harness disposal recovers actual endpoint
        // owners, including the postcommit opaque predecessor.
        drop(control);
        drop(callback);
        assert_eq!(weak.strong_count(), 0);
        for _ in 0..4 {
            let _phase = root.tick(true);
        }
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}
