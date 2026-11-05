//! Independently linked test of the real ordinary streamed publication boundary.
use hex_audio::{
    audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags},
    prepared::{AdmittedPublishError, FreshRuntimeCancellationResult, PublicationPolicy},
    worker_fixture,
};
use hex_engine::{
    StreamSourceKey, StreamedSourceMetadata,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
    playback::{Playback, PlaybackCommand, PlaybackHandoffRejection},
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{SourceDefinition, StreamConfig, StreamReadinessReservation, StreamRuntime};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Acquire},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Wav(PathBuf);
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn render(backend: &mut worker_fixture::HeadlessBackend) -> [[f32; 2]; 8] {
    let mut output = [[0.0; 2]; 8];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| result = backend.render(&mut output));
    assert!(result.is_ok());
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    output
}
#[test]
fn ordinary_stream_publication_preserves_receipts_pcm_cancellation_and_custody() {
    ordinary_stream_publication(false);
}

#[test]
fn compensated_stream_publication_preserves_receipts_pcm_cancellation_and_custody() {
    ordinary_stream_publication(true);
}

fn ordinary_stream_publication(compensated: bool) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = Wav(std::env::temp_dir().join(format!(
        "hex-audio-boundary-{}-{stamp}.wav",
        std::process::id()
    )));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&68_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&192_000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&32_u32.to_le_bytes());
    for _ in 0..16 {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    fs::write(&file.0, wav).unwrap();
    let limits =
        VerifiedWavReaderLimits::new(WavReaderLimits::new(1024 * 1024, 8, 32, 32768), 1024 * 1024);
    let fingerprint = *VerifiedWavReader::open(&file.0, limits)
        .unwrap()
        .fingerprint();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: file.0.clone(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    if compensated {
        let Created::Track(midi) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("midi track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: midi,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 60,
                velocity: 90,
                channel: 0,
            })
            .unwrap();
        let limiter = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: limiter,
            })
            .unwrap();
    }
    let expected = if compensated {
        [[0.0; 2]; 8]
    } else {
        [[0.25; 2]; 8]
    };
    let metadata = |generation| {
        StreamedSourceMetadata::new(
            StreamSourceKey::new(generation, asset),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )
        .unwrap()
    };
    let source = metadata(11);
    let store = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(&store, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let compile = |source: StreamedSourceMetadata| {
        let generation = source.key().generation();
        let sources = BTreeMap::from([(
            asset,
            hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(source)),
        )]);
        let prepared =
            hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
                editor.project(),
                &sources,
                generation,
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                100_000,
                &cancel,
            )
            .unwrap();
        Playback::try_from_prepared_with_device_resources(Arc::new(prepared), &resources).unwrap()
    };
    let mut stream = StreamRuntime::new(
        StreamConfig::with_pool_pages(4),
        vec![SourceDefinition::new(
            file.0.clone(),
            source.clone(),
            limits,
        )],
    )
    .unwrap();
    stream.wait_until_ready(Duration::from_secs(2)).unwrap();
    let pair = stream.split().ok().unwrap();
    let (mut output, mut backend) = worker_fixture::open_after_retirement(48_000, false).unwrap();
    let control = output.publication.control().unwrap();
    control.set_audio_diagnostics(AudioDiagnosticsFlags::TRANSPORT);
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    for (extents, bytes) in [(usize::MAX, 4096), (3, 1)] {
        let mut refused = false;
        let counts = allocation_counter::measure(|| {
            refused = control
                .configure_ordinary_streaming_on_control(
                    hex_audio::AudioCandidateDomainReservation {
                        max_retained_data_bytes: 8 * 1024 * 1024,
                        max_construction_peak_data_bytes: 8 * 1024 * 1024,
                    },
                    extents,
                    bytes,
                )
                .is_err()
        });
        assert!(refused);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    control
        .configure_ordinary_streaming_on_control(
            hex_audio::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    assert!(matches!(
        control.publish_with_admission_and_source_on_control(
            compile(source.clone()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(())
        ),
        Err(AdmittedPublishError::Publication(_))
    ));
    assert_eq!(control.submitted_revision(), 0);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pair,
            StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    assert!(control.audio_candidate_domain_is_sealed(domain));
    assert!(!control.audio_candidate_domain_endpoint_is_installed(domain));
    assert_eq!(render(&mut backend), [[0.0; 2]; 8]);
    assert!(control.audio_candidate_domain_endpoint_is_installed(domain));
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(source.clone()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .ok()
        .unwrap();
    let mut client = output.take_client().unwrap();
    let play = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let samples = render(&mut backend);
        let status = client.status().unwrap();
        if status.applied_revision.load(Acquire) == revision {
            assert_eq!(samples, expected);
            break;
        }
        assert_eq!(samples, [[0.0; 2]; 8]);
        assert_eq!(status.performance_frame.load(Acquire), 0);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        client
            .status()
            .unwrap()
            .stream_applied_command_prefix
            .load(Acquire),
        play
    );
    assert_eq!(client.status().unwrap().performance_frame.load(Acquire), 8);
    assert_eq!(client.status().unwrap().position.load(Acquire), 8);
    assert!(
        std::iter::from_fn(|| client.take_audio_diagnostic()).any(|event| matches!(
            event.kind,
            AudioDiagnosticKind::CommandDequeued {
                ticket,
                action: hex_engine::PlaybackAction::Transport(PlaybackCommand::Play),
                ..
            } if ticket == play
        ))
    );
    if compensated {
        let mut heard = false;
        for _ in 0..40 {
            heard |= render(&mut backend)
                .iter()
                .flatten()
                .any(|sample| *sample != 0.0);
        }
        assert!(heard, "compensated mix must reach the output");
    }
    let seek = client
        .command_with_ticket(PlaybackCommand::PlayFrom(0))
        .unwrap();
    assert_eq!(render(&mut backend), expected);
    assert_eq!(
        client
            .status()
            .unwrap()
            .stream_applied_command_prefix
            .load(Acquire),
        seek
    );
    let control = output.publication.control().unwrap();
    assert!(
        control
            .publish_with_admission_and_source_on_control(
                compile(metadata(12)),
                PublicationPolicy::InheritRuntime,
                context,
                |_| Ok::<_, ()>(())
            )
            .is_err()
    );
    assert_eq!(control.submitted_revision(), revision);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled = control
        .publish_with_admission_and_source_on_control(
            compile(source.clone()),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .ok()
        .unwrap();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(2));
    assert_eq!(
        control.cancel_fresh_runtime_publication(cancelled),
        FreshRuntimeCancellationResult::Cancelled
    );
    render(&mut backend);
    assert_eq!(
        client.status().unwrap().applied_revision.load(Acquire),
        revision
    );
    assert_eq!(
        client.status().unwrap().rejected_revision(),
        Some((cancelled, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    assert!(matches!(
        control.publish_with_admission_and_source_on_control(
            compile(source.clone()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Err::<(), _>("refused")
        ),
        Err(AdmittedPublishError::Admission { .. })
    ));
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let wrong = control
        .publish_with_admission_and_source_on_control(
            compile(source),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 8,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .ok()
        .unwrap();
    render(&mut backend);
    assert_eq!(
        client.status().unwrap().applied_revision.load(Acquire),
        revision
    );
    assert_eq!(
        client.status().unwrap().rejected_revision(),
        Some((wrong, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    assert!(control.retire_audio_candidate_domain(domain));
    assert!(!control.audio_candidate_domain_endpoint_is_installed(domain));
    assert!(control.recover_audio_candidate_domain(domain).is_none());
    control.request_shutdown();
    loop {
        render(&mut backend);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    assert!(!control.audio_candidate_domain_endpoint_is_installed(domain));
    client.close();
    backend.close();
    output.publication.close();
}
