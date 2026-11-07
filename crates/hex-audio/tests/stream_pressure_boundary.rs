//! Real callback and worker coverage for successor/current page-pool pressure.
#![cfg(all(feature = "allocation-probe", feature = "stream-test-support"))]

use hex_audio::{prepared::PublicationPolicy, worker_fixture};
use hex_engine::{
    StreamSourceKey, StreamedSourceMetadata,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
    playback::{Playback, PlaybackCommand},
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamReadinessReservation, StreamRuntime,
};
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
impl Wav {
    fn new(frames: u32, sample: i16, label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-pressure-{}-{stamp}-{label}.wav",
            std::process::id()
        ));
        let size = frames * 4;
        let mut bytes = Vec::with_capacity(44 + size as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&192_000_u32.to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&size.to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&sample.to_le_bytes());
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
    fn definition(&self, asset: hex_project::AssetId, frames: u64) -> SourceDefinition {
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, frames, 32, 32768),
            1024 * 1024,
        );
        let fingerprint = *VerifiedWavReader::open(&self.0, limits)
            .unwrap()
            .fingerprint();
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(11, asset),
            48_000,
            48_000,
            frames,
            frames,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )
        .unwrap();
        SourceDefinition::new(self.0.clone(), metadata, limits)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn compile(
    project: &Project,
    sources: &[StreamedSourceMetadata],
    store: &ResidentDeviceAssets,
) -> Playback {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 1_000_000, &cancel).unwrap();
    let sources = sources
        .iter()
        .cloned()
        .map(|source| {
            (
                source.key().asset(),
                hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(source)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let revision =
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            project,
            &sources,
            11,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancel,
        )
        .unwrap();
    Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources).unwrap()
}
fn render(backend: &mut worker_fixture::HeadlessBackend, output: &mut [[f32; 2]]) {
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| result = backend.render(output));
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
}

#[test]
fn held_successor_yields_to_current_page_and_then_hands_off() {
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 3) as u64;
    let a_wav = Wav::new(frames as u32, 8192, "a");
    let b_wav = Wav::new(frames as u32, 16384, "b");
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported {
        track: a_track,
        asset: a_asset,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "a".into(),
            path: a_wav.0.clone(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("a")
    };
    let Created::Imported {
        track: b_track,
        asset: b_asset,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "b".into(),
            path: b_wav.0.clone(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("b")
    };
    editor
        .execute(Command::Mute {
            track: b_track,
            muted: true,
        })
        .unwrap();
    let a_project = editor.project().clone();
    editor
        .execute(Command::Mute {
            track: a_track,
            muted: true,
        })
        .unwrap();
    editor
        .execute(Command::Mute {
            track: b_track,
            muted: false,
        })
        .unwrap();
    let b_project = editor.project().clone();
    let a = a_wav.definition(a_asset, frames);
    let b = b_wav.definition(b_asset, frames);
    let sources = vec![a.metadata.clone(), b.metadata.clone()];
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(PageKey {
        source: b.metadata.key(),
        page_index: 1,
    });
    let mut stream =
        StreamRuntime::new_with_page_gate(StreamConfig::with_pool_pages(3), vec![a, b], gate)
            .unwrap();
    stream.wait_until_ready(Duration::from_secs(2)).unwrap();
    let pair = stream.split().ok().unwrap();
    let store = ResidentDeviceAssets::default();
    let (mut output, mut backend) = worker_fixture::open(48_000).unwrap();
    let control = output.publication.control().unwrap();
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
            hex_audio::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pair,
            StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("domain"));
    let mut block = [[0.0; 2]; 256];
    render(&mut backend, &mut block);
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let first = control
        .publish_with_admission_and_source_on_control(
            compile(&a_project, &sources, &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("first publication"));
    let mut client = output.take_client().unwrap();
    client.command(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while client.status().unwrap().position.load(Acquire) < hex_engine::STREAM_PAGE_FRAMES as u64 {
        render(&mut backend, &mut block);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        client.status().unwrap().applied_revision.load(Acquire),
        first
    );
    let control = output.publication.control().unwrap();
    let second = control
        .publish_with_admission_and_source_on_control(
            compile(&b_project, &sources, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("second publication"));
    let mut held = false;
    let mut yielded = false;
    let mut attempts = 0usize;
    while !yielded {
        let before = client.status().unwrap().performance_frame.load(Acquire);
        render(&mut backend, &mut block);
        let status = client.status().unwrap();
        let committed = if status.stream_buffering.load(Acquire) {
            status.stream_committed_frames.load(Acquire) as usize
        } else {
            block.len()
        };
        assert_eq!(
            status.performance_frame.load(Acquire),
            before + committed as u64
        );
        assert_eq!(status.applied_revision.load(Acquire), first);
        assert!(status.rejected_revision().is_none());
        for (i, frame) in block.iter().enumerate() {
            assert_eq!(*frame, [if i < committed { 0.25 } else { 0.0 }; 2]);
        }
        held |= entered.try_recv().is_ok();
        yielded = backend.stream_successor_yielded_on_control().unwrap();
        attempts += 1;
        assert!(
            attempts <= 200,
            "current window did not yield the held successor"
        );
        assert!(Instant::now() < deadline, "capacity yield did not occur");
        if !yielded {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    if !held {
        held = entered.recv_timeout(Duration::from_millis(100)).is_ok();
    }
    assert!(held, "successor worker did not reach the held request");
    release.send(()).unwrap();
    while client.status().unwrap().applied_revision.load(Acquire) != second {
        let before = client.status().unwrap().performance_frame.load(Acquire);
        render(&mut backend, &mut block);
        let status = client.status().unwrap();
        let committed = if status.stream_buffering.load(Acquire) {
            status.stream_committed_frames.load(Acquire) as usize
        } else {
            block.len()
        };
        assert_eq!(
            status.performance_frame.load(Acquire),
            before + committed as u64
        );
        let revision = status.applied_revision.load(Acquire);
        assert!(revision == first || revision == second);
        assert!(status.rejected_revision().is_none());
        let sample = if revision == first { 0.25 } else { 0.5 };
        for (i, frame) in block.iter().enumerate() {
            assert_eq!(*frame, [if i < committed { sample } else { 0.0 }; 2]);
        }
        assert!(
            Instant::now() < deadline,
            "replacement did not finish after yield"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(block, [[0.5; 2]; 256]);
    let control = output.publication.control().unwrap();
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    loop {
        render(&mut backend, &mut block);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    client.close();
    backend.close();
    output.publication.close();
}
