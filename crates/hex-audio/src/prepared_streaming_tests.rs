use super::*;
use hex_project::{AssetSnapshot, ClipSnapshot, TrackContentSnapshot, TrackSnapshot};
use std::{
    collections::BTreeMap,
    io::Write,
    thread,
    time::{Duration, Instant},
};

static FILE_ID: AtomicU64 = AtomicU64::new(1);

fn owning_audio_stage_fixture() -> (
    hex_project::Project,
    hex_engine::OwnedAudioInputs,
    hex_engine::AudioSourceToken,
) {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "owned.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
    let source = hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 8])
        .ok()
        .unwrap();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(asset, source)],
        hex_engine::AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let token = bank.source_token(asset).unwrap();
    (editor.project().clone(), bank, token)
}
fn owning_constructor_available(test: &str) -> bool {
    match hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]; 8]) {
        Ok(_) => {
            eprintln!("hex preparation host evidence: {test}; branch=supported-constructor");
            true
        }
        Err(refusal) => {
            assert_eq!(
                refusal.reason,
                hex_engine::AudioStorageError::UnsupportedConstructor
            );
            eprintln!("hex preparation host evidence: {test}; branch=unsupported-refusal");
            false
        }
    }
}
#[test]
fn actual_native_pending_host_retains_audio_stage_through_handle_abandonment_cancel_and_recovery() {
    if !owning_constructor_available("pending_host_cancel_recovery") {
        return;
    }
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (project, bank, source_token) = owning_audio_stage_fixture();
    let original_capacity = bank.capacity_report();
    let token = root
        .begin_audio_arrangement(
            hex_engine::AudioArrangementInput::new(&project, 1, bank),
            48_000,
        )
        .ok()
        .unwrap();
    let lifecycle_anchor = root.clone(); // existing root custody, not a token registry
    drop(root);
    assert_eq!(
        lifecycle_anchor.audio_arrangement_phase(),
        Some(hex_engine::AudioPreparationPhase::ArrangementSources)
    );
    assert!(lifecycle_anchor.cancel_audio_arrangement(&token));
    assert!(lifecycle_anchor.recover_audio_arrangement(&token).is_none());
    let _ = lifecycle_anchor.tick(false);
    assert_eq!(
        lifecycle_anchor.audio_arrangement_phase(),
        Some(hex_engine::AudioPreparationPhase::Recoverable)
    );
    let bank = lifecycle_anchor.recover_audio_arrangement(&token).unwrap();
    assert_eq!(bank.capacity_report(), original_capacity);
    assert!(bank.source_view(&source_token).is_ok());
    assert_eq!(bank.owning_handle_count(), 1);
    assert!(lifecycle_anchor.recover_audio_arrangement(&token).is_none());
    eprintln!(
        "hex preparation host evidence: pending_host_cancel_recovery; supported-controls=completed"
    );
}
#[test]
fn terminal_close_keeps_abandoned_bank_pending_and_refuses_replacement_without_false_cleanup() {
    if !owning_constructor_available("terminal_close_abandonment") {
        return;
    }
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    {
        let (project, bank, _) = owning_audio_stage_fixture();
        let _token = root
            .begin_audio_arrangement(
                hex_engine::AudioArrangementInput::new(&project, 1, bank),
                48_000,
            )
            .ok()
            .unwrap();
    } // lexical token abandonment has no owner destructor or join
    assert_eq!(root.tick(true), hex_stream::MediaCleanupPhase::Pending);
    assert_eq!(
        root.audio_arrangement_phase(),
        Some(hex_engine::AudioPreparationPhase::Recoverable)
    );
    let (other, bank, source) = owning_audio_stage_fixture();
    let capacity = bank.capacity_report();
    let refusal = root
        .begin_audio_arrangement(
            hex_engine::AudioArrangementInput::new(&other, 2, bank),
            48_000,
        )
        .err()
        .unwrap();
    assert_eq!(refusal.reason, hex_engine::AudioStorageError::Capacity);
    assert_eq!(refusal.inputs.capacity_report(), capacity);
    assert!(refusal.inputs.source_view(&source).is_ok());
    assert_eq!(root.tick(true), hex_stream::MediaCleanupPhase::Pending);
    eprintln!(
        "hex preparation host evidence: terminal_close_abandonment; supported-controls=completed"
    );
}
pub(super) struct FixtureFile(std::path::PathBuf);
#[test]
fn actual_native_pending_slot_hosts_connected_session_source_stage_and_control_recovery() {
    if !owning_constructor_available("connected_session_source_stage") {
        return;
    }
    let (project, bank, source_token) = owning_audio_stage_fixture();
    let track = project.tracks()[0].id();
    let asset = project.assets().next().unwrap().id();
    let mut editor = hex_project::Editor::new(project, 16).unwrap();
    let hex_project::Created::Scene(scene) = editor
        .execute(hex_project::Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(hex_project::Command::CreateSessionAudioClip {
            track,
            scene,
            name: "session audio".into(),
            asset,
            source_start: 1,
            source_end: 7,
        })
        .unwrap();
    let original_capacity = bank.capacity_report();
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let token = root
        .begin_audio_arrangement(
            hex_engine::AudioArrangementInput::new(editor.project(), 1, bank)
                .with_session_sources(),
            48_000,
        )
        .ok()
        .unwrap();
    assert_eq!(
        root.audio_arrangement_phase(),
        Some(hex_engine::AudioPreparationPhase::ArrangementAndSessionSources)
    );
    assert!(root.cancel_audio_arrangement(&token));
    assert!(root.recover_audio_arrangement(&token).is_none());
    let _ = root.tick(false);
    let bank = root.recover_audio_arrangement(&token).unwrap();
    assert_eq!(bank.capacity_report(), original_capacity);
    assert_eq!(bank.owning_handle_count(), 1);
    assert!(bank.source_view(&source_token).is_ok());
    eprintln!(
        "hex preparation host evidence: connected_session_source_stage; supported-controls=completed"
    );
}
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
                tracked_claim: None,
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

pub(super) fn facade_input(id: u64) -> (crate::stream_preparation::CandidateInput, FixtureFile) {
    use crate::stream_preparation::*;
    let (request, file) = request(id);
    let StreamPublicationRequest {
        playback,
        verified,
        source,
        ..
    } = request;
    let key = playback
        .prepared_revision()
        .unwrap()
        .streamed_sources()
        .next()
        .unwrap()
        .key();
    drop(verified);
    let StreamPublicationSource::New(input) = source else {
        panic!("fixture owns domain");
    };
    let hex_stream::MediaDomainInput {
        control,
        endpoint,
        retained_leases,
        ..
    } = *input;
    drop(retained_leases);
    let (pages, lease) = endpoint.into_endpoint().into_parts();
    drop(lease);
    assert!(
        control
            .shutdown_reported(pages)
            .unwrap()
            .successful_report()
            .is_some()
    );
    let mut path = file.0.clone();
    path.reserve(4096);
    let mut paths = Vec::with_capacity(7);
    paths.push((
        key,
        path,
        hex_stream::VerifiedWavReaderLimits::new(
            hex_stream::WavReaderLimits::new(1024 * 1024, 100_000, 128, 4096),
            1024,
        ),
    ));
    // Fixture-selected data reservations, never production admission defaults.
    (
        CandidateInput {
            playback,
            paths,
            identity: CandidateIdentity {
                format_version: 1,
                project_generation: id,
                editor_revision: 1,
            },
            limits: PreparationLimits {
                stream: hex_stream::StreamConfig::with_pool_pages(4),
                compact: hex_stream::CompactCopyReservations {
                    max_copy_bytes: 1024 * 1024,
                    max_overlap_bytes: 4 * 1024 * 1024,
                    other_retained_bytes: 0,
                    other_construction_bytes: 0,
                },
                max_construction_data_bytes: 16 * 1024 * 1024,
                max_prepared_data_bytes: 16 * 1024 * 1024,
                readiness_extents: 3,
            },
        },
        file,
    )
}
fn facade_wait(
    root: &NativeStreamRoot,
    token: crate::stream_preparation::PreparationToken,
    state: crate::stream_preparation::PreparationState,
) -> crate::stream_preparation::PreparationSnapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        root.tick(false);
        let status = root.preparation_snapshot(token).unwrap();
        if status.state == state {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "facade {:?}, reason {:?}",
            status.state,
            status.reason
        );
        thread::yield_now();
    }
}
#[test]
fn facade_real_catalog_publish_preserves_original_paths_and_installed_buffering_contract() {
    use crate::stream_preparation::*;
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (input, _file) = facade_input(81);
    let identity = input.identity;
    let addresses = (
        input.paths.as_ptr() as usize,
        input.paths.capacity(),
        input.paths[0].1.as_os_str().as_encoded_bytes().as_ptr() as usize,
        input.paths[0].1.capacity(),
    );
    let token = root.begin_preparation(input, 48_000).unwrap();
    let status = facade_wait(&root, token, PreparationState::CatalogPrepared);
    assert_eq!(root.preparation_original_addresses(token), Some(addresses));
    assert!(status.report.construction.unwrap().worker.finalized);
    assert!(status.report.prepared_data_bound_bytes.unwrap() >= status.report.readiness_bytes);
    let (other, _other_file) = facade_input(82);
    let other_ptr = other.paths.as_ptr();
    let refusal = root.begin_preparation(other, 48_000).unwrap_err();
    assert_eq!(refusal.input.paths.as_ptr(), other_ptr);
    assert_eq!(refusal.reason, "stream preparation slot is occupied");
    drop(refusal);
    let revision = root
        .publish_preparation(
            &mut control,
            token,
            identity,
            1,
            PublicationPolicy::InstallFreshRuntime,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        root.preparation_snapshot(token).unwrap().published_revision,
        Some(revision)
    );
    assert!(!root.cancel_preparation(token)); // Cannot revoke queued acceptance.
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[9.0; 2]; 256];
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        callback.render(&mut output).unwrap();
        if control.status.performance_frame.load(Relaxed) > 0 {
            break;
        }
        assert_eq!(output, [[0.0; 2]; 256]);
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(output.iter().any(|s| *s != [0.0; 2]));
    assert!(root.consume_preparation(token));
    root.tick(false);
    assert!(root.preparation_snapshot(token).is_none());
    drop(control);
    drop(callback);
    for _ in 0..4 {
        root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}
#[test]
fn facade_stale_context_missing_file_and_capacity_refusal_recover_exact_owners() {
    use crate::stream_preparation::*;
    for failure in 0..4 {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (mut input, file) = facade_input(90 + failure);
        let identity = input.identity;
        if failure == 1 {
            std::fs::remove_file(&file.0).unwrap();
        }
        if failure == 2 {
            input.limits.max_prepared_data_bytes = 1;
        }
        if failure == 3 {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&file.0)
                .unwrap()
                .write_all(&[1, 2, 3, 4])
                .unwrap();
        }
        let addresses = (
            input.paths.as_ptr() as usize,
            input.paths.capacity(),
            input.paths[0].1.as_os_str().as_encoded_bytes().as_ptr() as usize,
            input.paths[0].1.capacity(),
        );
        let token = root.begin_preparation(input, 48_000).unwrap();
        if failure == 0 {
            facade_wait(&root, token, PreparationState::CatalogPrepared);
            let stale = CandidateIdentity {
                editor_revision: identity.editor_revision + 1,
                ..identity
            };
            assert_eq!(
                root.publish_preparation(
                    &mut control,
                    token,
                    stale,
                    1,
                    PublicationPolicy::InheritRuntime
                ),
                Err("stream preparation context stale")
            );
        }
        let report = facade_wait(&root, token, PreparationState::Outcome).report;
        assert_eq!(root.preparation_original_addresses(token), Some(addresses));
        if failure == 1 || failure == 3 {
            assert!(matches!(
                report.construction_cleanup,
                Some(hex_stream::ConstructionCleanup::Joined(_))
            ));
        } else {
            assert!(report.shutdown.unwrap().successful_report().is_some());
        }
        assert_eq!(control.status.applied_revision.load(Relaxed), 0);
        assert!(root.consume_preparation(token));
        root.tick(false);
        assert!(root.preparation_snapshot(token).is_none());
        assert!(!root.cancel_preparation(token));
        drop(control);
        drop(callback);
        for _ in 0..4 {
            root.tick(true);
        }
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}
#[test]
fn facade_version_and_queued_cancel_refuse_without_ownership_or_ui_work() {
    use crate::stream_preparation::*;
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (mut input, _file) = facade_input(96);
    input.identity.format_version = 2;
    let ptr = input.paths.as_ptr();
    let mut refusal = root.begin_preparation(input, 48_000).unwrap_err();
    assert_eq!(refusal.input.paths.as_ptr(), ptr);
    refusal.input.identity.format_version = 1;
    let token = root.begin_preparation(*refusal.input, 48_000).unwrap();
    let measured = allocation_counter::measure(|| assert!(root.cancel_preparation(token)));
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0)
    );
    let status = facade_wait(&root, token, PreparationState::Outcome);
    assert!(status.report.construction.is_none());
    assert!(matches!(
        status.report.construction_cleanup,
        Some(hex_stream::ConstructionCleanup::NotStarted {
            allocated_pages: 0,
            reclaimed_pages: 0
        })
    ));
    assert!(root.consume_preparation(token));
    root.tick(false);
    drop(control);
    drop(callback);
    for _ in 0..4 {
        root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[test]
fn facade_abandoned_output_retains_request_lease_and_status_until_both_closures_return() {
    let _serialized = crate::lifecycle::tests::LEASE_TEST.lock().unwrap();
    let lease = crate::lifecycle::Lease::acquire().unwrap();
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (input, _file) = facade_input(99);
    let token = root.begin_preparation(input, 48_000).unwrap();
    root.tick(false); // Actual pool/transport allocated and real worker started.
    assert!(matches!(
        root.preparation_snapshot(token).unwrap().state,
        crate::stream_preparation::PreparationState::Constructing
    ));
    crate::prepared::observe_status_disposal(Arc::as_ptr(&control.status) as usize);
    crate::lifecycle::register_stream_root(root.clone());
    let (data, errors, terminal) =
        crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
    let owner = crate::lifecycle::StreamOwner::prepared_custody(lease, None, Some(root), terminal);
    drop(control);
    drop(owner); // No ordinary root/handle survives in this test.
    assert!(crate::lifecycle::Lease::acquire().is_err());
    let (data_thread, measurement) = thread::spawn(move || {
        let id = thread::current().id();
        (id, allocation_counter::measure(|| drop(data)))
    })
    .join()
    .unwrap();
    assert_eq!(
        (
            measurement.count_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0)
    );
    assert!(crate::prepared::observed_status_disposal().is_none());
    assert!(crate::lifecycle::Lease::acquire().is_err());
    let (error_thread, measurement) = thread::spawn(move || {
        let id = thread::current().id();
        (id, allocation_counter::measure(|| drop(errors)))
    })
    .join()
    .unwrap();
    assert_eq!(
        (
            measurement.count_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0)
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let replacement = loop {
        if let Ok(lease) = crate::lifecycle::Lease::acquire() {
            break lease;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    };
    let disposed = crate::prepared::observed_status_disposal().unwrap();
    assert_ne!(disposed, thread::current().id());
    assert_ne!(disposed, data_thread);
    assert_ne!(disposed, error_thread);
    drop(replacement);
}

#[test]
fn facade_injected_reserve_and_overflow_leave_originals_then_control_disposes_graph() {
    use crate::stream_preparation::*;
    for overflow in [false, true] {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (mut input, _file) = facade_input(104);
        let weak = Arc::downgrade(input.playback.prepared_revision().unwrap());
        if overflow {
            input.limits.compact.other_retained_bytes = usize::MAX;
        } else {
            fail_definition_reserve();
        }
        let addresses = (
            input.paths.as_ptr() as usize,
            input.paths.capacity(),
            input.paths[0].1.as_os_str().as_encoded_bytes().as_ptr() as usize,
            input.paths[0].1.capacity(),
        );
        let token = root.begin_preparation(input, 48_000).unwrap();
        let snapshot = facade_wait(&root, token, PreparationState::Outcome);
        assert!(snapshot.report.construction.is_none());
        assert_eq!(root.preparation_original_addresses(token), Some(addresses));
        assert_eq!(weak.strong_count(), 2);
        let measured = allocation_counter::measure(|| assert!(root.consume_preparation(token)));
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0)
        );
        assert_eq!(weak.strong_count(), 2); // UI's metadata operation drops nothing.
        let measured = allocation_counter::measure(|| {
            root.tick(false);
        });
        assert!(measured.bytes_current < 0);
        assert_eq!(weak.strong_count(), 0);
        drop(control);
        drop(callback);
        for _ in 0..4 {
            root.tick(true);
        }
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}
#[test]
fn facade_pending_publication_and_legacy_id_collision_keep_candidate_retryable() {
    use crate::stream_preparation::*;
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (existing, _first_file) = request(1);
    root.publish(&mut control, existing).unwrap();
    let (input, _file) = facade_input(106);
    let identity = input.identity;
    let token = root.begin_preparation(input, 48_000).unwrap();
    facade_wait(&root, token, PreparationState::CatalogPrepared);
    assert_eq!(
        root.publish_preparation(
            &mut control,
            token,
            identity,
            1,
            PublicationPolicy::InstallFreshRuntime
        ),
        Err("playback publication already pending")
    );
    assert_eq!(
        root.preparation_snapshot(token).unwrap().state,
        PreparationState::CatalogPrepared
    );
    callback.begin_block();
    assert!(
        root.publish_preparation(
            &mut control,
            token,
            identity,
            1,
            PublicationPolicy::InstallFreshRuntime
        )
        .unwrap()
        .is_some()
    );
    callback.begin_block();
    assert!(root.consume_preparation(token));
    root.tick(false);
    drop(control);
    drop(callback);
    for _ in 0..5 {
        root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[test]
fn facade_pending_cancel_wins_catalog_result_and_stale_token_cannot_cancel_replacement() {
    use crate::stream_preparation::*;
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let (input, _file) = facade_input(107);
    let identity = input.identity;
    let token = root.begin_preparation(input, 48_000).unwrap();
    root.tick(false);
    // Let the real small worker queue verification, but do not poll/finish it.
    thread::sleep(Duration::from_millis(20));
    assert!(root.cancel_preparation(token));
    let outcome = facade_wait(&root, token, PreparationState::Outcome);
    assert!(matches!(
        outcome.report.construction_cleanup,
        Some(hex_stream::ConstructionCleanup::Joined(_))
    ));
    assert_eq!(
        root.publish_preparation(
            &mut control,
            token,
            identity,
            1,
            PublicationPolicy::InheritRuntime
        ),
        Err("stream preparation cancelled")
    );
    assert!(root.consume_preparation(token));
    root.tick(false);
    let (input, _next_file) = facade_input(107);
    let next = root.begin_preparation(input, 48_000).unwrap();
    assert_ne!(next, token);
    assert!(!root.cancel_preparation(token));
    assert_eq!(root.current_preparation_snapshot().unwrap().token, next);
    assert!(root.cancel_preparation(next));
    facade_wait(&root, next, PreparationState::Outcome);
    assert!(root.consume_preparation(next));
    root.tick(false);
    drop(control);
    drop(callback);
    for _ in 0..4 {
        root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[test]
fn owned_origin_refuses_legacy_facades_without_losing_original_input() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "owned".into(),
        })
        .unwrap();
    let owner = hex_engine::PreparedRevision::prepare(
        editor.project(),
        &std::collections::BTreeMap::new(),
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap()
    .into_owned_shared();
    let alias = owner.clone_on_control();
    let playback = Playback::try_from_owned_prepared_with_session(owner)
        .ok()
        .unwrap();
    let (mut input, _file) = facade_input(108);
    input.playback = playback;
    let identity = (
        input.paths.as_ptr(),
        input.paths.capacity(),
        input.paths[0].1.as_os_str().as_encoded_bytes().as_ptr(),
        input.paths[0].1.capacity(),
    );
    let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
    let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
    let refusal = root.begin_preparation(input, 48_000).err().unwrap();
    assert_eq!(
        refusal.reason,
        "owned playback requires the typed publication bridge"
    );
    assert_eq!(
        (
            refusal.input.paths.as_ptr(),
            refusal.input.paths.capacity(),
            refusal.input.paths[0]
                .1
                .as_os_str()
                .as_encoded_bytes()
                .as_ptr(),
            refusal.input.paths[0].1.capacity()
        ),
        identity
    );
    assert!(refusal.input.playback.same_prepared_owner(&alias));
    let error = control.publish(refusal.input.playback).err().unwrap();
    assert_eq!(
        error.reason,
        "owned playback requires the typed publication bridge"
    );
    assert!(error.playback.same_prepared_owner(&alias));
    let mut frames = [[9.0; 2]; 64];
    callback.render(&mut frames).unwrap();
    assert_eq!(control.status.applied_revision.load(Relaxed), 0);
    drop(error);
    drop(control);
    drop(callback);
    for _ in 0..4 {
        root.tick(true);
    }
    assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
}

#[cfg(feature = "allocation-probe")]
#[test]
fn owned_borrowed_resident_render_and_identity_queries_allocate_and_free_nothing() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { asset, .. } = editor
        .execute(hex_project::Command::ImportPreparedAudioTrack {
            name: "resident".into(),
            path: "resident.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 512,
            playback_frames: 512,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.25]; 512]).unwrap(),
    )]);
    let owner = hex_engine::PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap()
    .into_owned_shared();
    let alias = owner.clone_on_control();
    let mut playback = Playback::try_from_owned_prepared_with_session(owner)
        .ok()
        .unwrap();
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        assert!(playback.same_prepared_owner(&alias));
        assert_eq!(playback.owned_metadata().unwrap().sample_rate(), 48_000);
        playback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().any(|frame| *frame != [0.0; 2]));
    // Headless borrowed rendering evidence, not native publication/readiness.
}
