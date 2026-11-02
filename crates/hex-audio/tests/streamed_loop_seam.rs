//! A streamed Arrangement loop seam reads its overrun page without callback allocation.
#![cfg(feature = "allocation-probe")]
use hex_audio::{prepared::PublicationPolicy, worker_fixture};
use hex_engine::{
    STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
    playback::{Playback, PlaybackCommand},
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

/// Two stream pages; the loop ends exactly on the page boundary, so the
/// seam's outgoing overrun lives only in the second page.
const FRAMES: usize = 2 * STREAM_PAGE_FRAMES;
const LOOP_START: u64 = 2_048;
const LOOP_END: u64 = STREAM_PAGE_FRAMES as u64;
const BLOCK: usize = 64;
const LEVEL: f32 = 0.25;

struct Wav(PathBuf);
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn render(backend: &mut worker_fixture::HeadlessBackend) -> [[f32; 2]; BLOCK] {
    let mut output = [[0.0; 2]; BLOCK];
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

fn dc_wav(path: &PathBuf) {
    let data = u32::try_from(FRAMES * 4).unwrap();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&192_000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for _ in 0..FRAMES * 2 {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    fs::write(path, wav).unwrap();
}

/// DC is continuous across the seam, so rendering the outgoing side from a
/// missing page would show as a dip below `LEVEL` after a wrap. The overrun
/// page is the page after the one holding the loop end, so the playhead's
/// bounded lookahead (#4075) loads it before the first wrap demands it: no
/// wrap holds the transport, and every frame plays at `LEVEL`.
#[test]
fn streamed_loop_seam_crossfade_is_continuous_and_callback_allocation_free() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = Wav(std::env::temp_dir().join(format!(
        "hex-audio-loop-seam-{}-{stamp}.wav",
        std::process::id()
    )));
    dc_wav(&file.0);
    let frames = FRAMES as u64;
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(1024 * 1024, frames, 32, STREAM_PAGE_FRAMES),
        1024 * 1024,
    );
    let fingerprint = *VerifiedWavReader::open(&file.0, limits)
        .unwrap()
        .fingerprint();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: file.0.clone(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::SetLoopRegion {
            start: LOOP_START,
            end: LOOP_END,
            enabled: true,
        })
        .unwrap();
    let source = StreamedSourceMetadata::new(
        StreamSourceKey::new(11, asset),
        48_000,
        48_000,
        frames,
        frames,
        hex_project::IDENTITY_CONVERSION,
        fingerprint,
    )
    .unwrap();
    let store = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(&store, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let sources = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(source.clone())),
    )]);
    let prepared =
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            editor.project(),
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
    let playback =
        Playback::try_from_prepared_with_device_resources(Arc::new(prepared), &resources).unwrap();
    let mut stream = StreamRuntime::new(
        StreamConfig::with_pool_pages(4),
        vec![SourceDefinition::new(file.0.clone(), source, limits)],
    )
    .unwrap();
    stream.wait_until_ready(Duration::from_secs(2)).unwrap();
    let pair = stream.split().ok().unwrap();
    let (mut output, mut backend) = worker_fixture::open_after_retirement(48_000, false).unwrap();
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
            3,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pair,
            StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    render(&mut backend);
    let revision = control
        .publish_with_admission_and_source_on_control(
            playback,
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .ok()
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut position = 0_u64;
    let mut wraps = 0;
    let mut held_at = Vec::new();
    while wraps < 3 {
        let samples = render(&mut backend);
        let status = client.status().unwrap();
        if status.applied_revision.load(Acquire) != revision {
            assert_eq!(samples, [[0.0; 2]; BLOCK]);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
            continue;
        }
        if status.position.load(Acquire) == position {
            assert_eq!(samples, [[0.0; 2]; BLOCK], "held chunk at {position}");
            held_at.push((wraps, position));
        } else {
            for frame in samples {
                assert_eq!(frame, [LEVEL; 2], "dip at timeline frame {position}");
                position += 1;
                if position == LOOP_END {
                    position = LOOP_START;
                    wraps += 1;
                }
            }
            assert_eq!(status.position.load(Acquire), position);
        }
        assert!(Instant::now() < deadline);
        // Leave the page worker real-time headroom between callbacks.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        held_at.is_empty(),
        "lookahead made the overrun resident before the wrap: {held_at:?}"
    );
    let control = output.publication.control().unwrap();
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    loop {
        render(&mut backend);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(Instant::now() < deadline + Duration::from_secs(2));
        std::thread::yield_now();
    }
    client.close();
    backend.close();
    output.publication.close();
}
