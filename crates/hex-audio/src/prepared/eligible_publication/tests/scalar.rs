//! Actual sealed automation file -> shared scalar worker/domain -> ordinary
//! native publication/render/return path. Headless allocation and custody
//! evidence only; this does not establish physical callback deadlines.
use super::*;
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId};
use hex_project::{AutomationValue, Command, Created, Editor, LoopRegion};
use hex_storage::automation_draft::{
    AppliedEvent, CaptureClock, CompletedAutomationTake, DraftClock, DraftProvenance,
    DraftReadLimits, DraftTarget, DraftTerminal, DraftWriter,
};
use std::{collections::BTreeMap, path::PathBuf};

struct DraftFile(PathBuf);
impl Drop for DraftFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn scalar_source(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> (DraftFile, hex_stream::ScalarSourceDefinition) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-native-scalar-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut writer = DraftWriter::create_with_clock(
        &path,
        project,
        DraftTarget::Device { device, target },
        DraftProvenance {
            sample_rate: 48_000,
            output_identity: 1,
            project_generation: 7,
            editor_revision: 3,
            audio_revision: 3,
            transport_epoch: 1,
            gesture_serial: 1,
        },
        CaptureClock {
            sample_rate: 48_000,
            loop_region: Some(LoopRegion::new(0, 12, true).unwrap()),
        },
    )
    .unwrap();
    writer
        .append(&[
            AppliedEvent {
                sequence: 1,
                performance_frame: 100,
                content_frame: 10,
                loop_pass: 0,
                value: AutomationValue::new(0.0).unwrap(),
            },
            AppliedEvent {
                sequence: 2,
                performance_frame: 102,
                content_frame: 0,
                loop_pass: 1,
                value: AutomationValue::new(0.75).unwrap(),
            },
        ])
        .unwrap();
    writer
        .finish(DraftTerminal::Finished(DraftClock {
            performance_frame: 110,
            content_frame: 8,
            loop_pass: 1,
        }))
        .unwrap();
    let source = CompletedAutomationTake::open_on_control(
        &path,
        DraftReadLimits {
            max_source_bytes: 128 * 1024,
        },
    )
    .unwrap()
    .into_prepared_on_control()
    .unwrap();
    let source = hex_stream::ScalarSourceDefinition::new(
        hex_engine::ScalarSourceKey::new(7, 1, 1).unwrap(),
        source,
        Arc::new(project.tempo_map().clone()),
    )
    .unwrap();
    (DraftFile(path), source)
}
fn compile(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
    source: hex_engine::ScalarSourceMetadata,
    audio: Option<&hex_engine::StreamedSourceMetadata>,
) -> Playback {
    let asset = project.assets().next().unwrap().id();
    let binding = hex_engine::ScalarAutomationBinding {
        device,
        target,
        source,
    };
    let revision = match audio {
        Some(audio) => {
            hex_engine::PreparedRevision::prepare_with_scalar_automation_and_prepared_sources(
                project,
                &BTreeMap::from([(
                    asset,
                    hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(audio.clone())),
                )]),
                audio.key().generation(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                binding,
            )
        }
        None => hex_engine::PreparedRevision::prepare_with_scalar_automation(
            project,
            &BTreeMap::from([(
                asset,
                hex_engine::Audio::new(48_000, vec![[0.25; 2]; 8192]).unwrap(),
            )]),
            Arc::new(hex_engine::SineTable::new()),
            None,
            binding,
        ),
    }
    .unwrap();
    assert!(
        revision.contains_streamed_sources(),
        "the scalar dependency alone must enter native page preflight"
    );
    Playback::from_prepared(Arc::new(revision))
}
fn measured_block(
    data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
) -> [Frame; 8] {
    let mut output = [[9.0; 2]; 8];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    output
}
#[test]
fn real_scalar_only_native_source_requires_catalog_and_pages_then_renders_and_joins_without_callback_allocations()
 {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for target in [
        DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        scalar_case(target, false);
    }
    // The same native endpoint, union readiness and retirement path must also
    // render a real streamed WAV beside the sealed scalar source.
    scalar_case(
        DeviceAutomationTarget::Parameter(hex_devices::utility::parameter::GAIN),
        true,
    );
}
fn scalar_case(target: DeviceAutomationTarget, mixed: bool) {
    let mut snapshot = readiness::project_with_frames(8192).snapshot();
    // This clip is resident in the scalar-only cases. Its recovery checkpoint
    // still uses an absolute media reference, as required by the draft codec.
    snapshot.assets[0].path = std::env::temp_dir().join("hex-native-scalar-resident.wav");
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("utility fixture device")
    };
    let project = editor.project();
    let wav = mixed.then(|| stream_fixture::Wav::with_frames(8192));
    let audio_definition = wav
        .as_ref()
        .map(|wav| wav.definition(project.assets().next().unwrap().id(), 11));
    let audio_metadata = audio_definition
        .as_ref()
        .map(|definition| definition.metadata.clone());
    let (_file, source) = scalar_source(project, device, target);
    let metadata = source.metadata().clone();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
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
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let refused = control
        .publish_with_admission_and_source_on_control(
            compile(
                project,
                device,
                target,
                metadata.clone(),
                audio_metadata.as_ref(),
            ),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .err()
        .unwrap();
    assert!(matches!(refused, AdmittedPublishError::Publication(_)));
    assert_eq!(control.submitted_revision(), 0);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));

    // Three cases have no PCM definition and prove scalar-only readiness. The
    // mixed case additionally supplies a real WAV to the same endpoint/pool.
    let mut runtime = hex_stream::StreamRuntime::spawn_with_scalar_sources(
        hex_stream::StreamConfig::with_pool_pages(4),
        audio_definition.into_iter().collect(),
        vec![source],
    )
    .unwrap();
    runtime
        .wait_until_ready(std::time::Duration::from_secs(2))
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            runtime.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|_| panic!("scalar-only domain registration"));
    assert!(control.audio_candidate_domain_is_sealed(domain));
    assert!(!control.audio_candidate_domain_endpoint_is_installed(domain));
    // Loan transfer is also a measured callback scope; no final owner drop is
    // hidden in the installation-only step.
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(control.audio_candidate_domain_endpoint_is_installed(domain));
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(project, device, target, metadata, audio_metadata.as_ref()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("scalar-only ordinary publication"));
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert!(control.status.stream_buffering.load(Acquire));
    assert_eq!(control.status.applied_revision.load(Acquire), 0);

    // Stop retains priority while the actual scalar graph is pending, even
    // when no ordinary command slot remains. It must not advance either clock.
    for _ in 0..COMMANDS {
        control.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    control.stop();
    assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert_eq!(control.status.position.load(Acquire), 0);

    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let first = loop {
        let output = measured_block(&mut data);
        if control.status.performance_frame.load(Acquire) == 8 {
            break output;
        }
        assert_eq!(output, [[0.0; 2]; 8]);
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert!(
        first
            .iter()
            .flatten()
            .all(|sample| sample.is_finite() && *sample > 0.25)
    );
    assert!(first.windows(2).all(|pair| pair[0] == pair[1]));
    let second = measured_block(&mut data);
    assert_eq!(
        &second[..2],
        &[[0.25; 2]; 2],
        "the actual internal coverage gap restores base"
    );
    let quiet = if target.is_host() { 0.00025 } else { 0.0 };
    for frame in &second[2..4] {
        for sample in frame {
            assert!(
                (*sample - quiet).abs() < 1.0e-8,
                "covered zero must use the shared target mapping"
            );
        }
    }
    assert_eq!(
        &second[4..],
        &[[0.25; 2]; 4],
        "outside coverage restores immutable base"
    );
    assert_eq!(control.status.performance_frame.load(Acquire), 16);

    assert!(control.retire_audio_candidate_domain(domain));
    control.reclaim();
    assert!(
        control.recover_audio_candidate_domain(domain).is_none(),
        "the live graph still retains its source association"
    );
    control.request_shutdown();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        measured_block(&mut data);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            assert_eq!(report.page_bytes, 4 * (4096 * 8 + 4096 * 4 + 64 * 8));
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
