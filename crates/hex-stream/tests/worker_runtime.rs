use hex_engine::{
    Audio, Clip, MediaPages, Project as EngineProject, Renderer, StreamSourceKey,
    StreamedSourceMetadata, Track,
};
use hex_media::{
    ResampleLimits, VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits, resample,
};
use hex_project::{AssetId, Command, Created, Editor, Project};
use hex_stream::{PageKey, SourceDefinition, StreamConfig, StreamRuntime};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "worker_runtime/readiness.rs"]
mod readiness_cases;

fn revision_for_stream(
    metadata: &StreamedSourceMetadata,
    session_only: bool,
) -> Arc<hex_engine::PreparedRevision> {
    let mut editor = Editor::new(Project::new(metadata.project_rate()).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, clip } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "binding".into(),
            path: "binding.wav".into(),
            source_sample_rate: metadata.source_rate(),
            source_frames: metadata.source_frames(),
            playback_frames: metadata.playback_frames(),
            conversion_version: metadata.conversion_version(),
        })
        .unwrap()
    else {
        panic!("import result");
    };
    assert_eq!(asset, metadata.key().asset());
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene result");
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "session binding".into(),
            asset,
            source_start: 0,
            source_end: metadata.playback_frames(),
        })
        .unwrap();
    if session_only {
        editor.execute(Command::RemoveClip(clip)).unwrap();
    }
    let bank = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata.clone())),
    )]);
    Arc::new(
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources(
            editor.project(),
            &bank,
            metadata.key().generation(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

fn next_fixture_id() -> u64 {
    FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
}

struct TestWav(PathBuf);

impl TestWav {
    fn stereo_pcm16(sample_rate: u32, frames: usize) -> (Self, Vec<[f32; 2]>) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-stream-{}-{stamp}-{}.wav",
            std::process::id(),
            next_fixture_id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create fixture");
        let data_bytes = frames.checked_mul(4).expect("fixture size");
        let riff_size = 36_usize.checked_add(data_bytes).expect("riff size");
        let mut bytes = Vec::with_capacity(riff_size + 8);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(riff_size as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.checked_mul(4).expect("byte rate").to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        let mut pcm = Vec::with_capacity(frames);
        for index in 0..frames {
            let left = ((index as i32 % 101) - 50) * 400;
            let right = (50 - (index as i32 % 97)) * 300;
            bytes.extend_from_slice(&(left as i16).to_le_bytes());
            bytes.extend_from_slice(&(right as i16).to_le_bytes());
            pcm.push([left as f32 / 32768.0, right as f32 / 32768.0]);
        }
        file.write_all(&bytes).expect("write fixture");
        file.sync_all().expect("flush fixture");
        (Self(path), pcm)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn stereo_float_with_nan(frames: usize) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-stream-float-{}-{stamp}-{}.wav",
            std::process::id(),
            next_fixture_id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create float fixture");
        let data_bytes = frames.checked_mul(8).expect("float fixture bytes");
        let riff_size = 36_usize.checked_add(data_bytes).expect("float RIFF size");
        file.write_all(b"RIFF").expect("RIFF");
        file.write_all(&(riff_size as u32).to_le_bytes())
            .expect("riff length");
        file.write_all(b"WAVEfmt ").expect("WAVE format");
        file.write_all(&16_u32.to_le_bytes())
            .expect("format length");
        file.write_all(&3_u16.to_le_bytes())
            .expect("float encoding");
        file.write_all(&2_u16.to_le_bytes()).expect("channel count");
        file.write_all(&48_000_u32.to_le_bytes())
            .expect("sample rate");
        file.write_all(&384_000_u32.to_le_bytes())
            .expect("byte rate");
        file.write_all(&8_u16.to_le_bytes()).expect("block align");
        file.write_all(&32_u16.to_le_bytes()).expect("bit depth");
        file.write_all(b"data").expect("data header");
        file.write_all(&(data_bytes as u32).to_le_bytes())
            .expect("data length");
        for index in 0..frames {
            let left = if index + 1 == frames {
                f32::NAN
            } else {
                index as f32 / 16.0
            };
            file.write_all(&left.to_le_bytes()).expect("left sample");
            file.write_all(&(-left).to_le_bytes())
                .expect("right sample");
        }
        file.sync_all().expect("flush float fixture");
        Self(path)
    }
}

impl Drop for TestWav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn asset_id(source_frames: usize, sample_rate: u32) -> AssetId {
    let mut editor = Editor::new(Project::new(sample_rate).expect("project"), 4).expect("editor");
    match editor
        .execute(Command::ImportAudioTrack {
            name: "stream source".into(),
            path: "stream-source.wav".into(),
            sample_rate,
            frames: source_frames as u64,
        })
        .expect("asset")
    {
        Created::Imported { asset, .. } => asset,
        other => panic!("unexpected import result: {other:?}"),
    }
}

fn reader_limits(max_frames: usize) -> VerifiedWavReaderLimits {
    VerifiedWavReaderLimits::new(
        WavReaderLimits::new(2 * 1024 * 1024, max_frames as u64, 128, 32_768),
        1024 * 1024,
    )
}

fn wait_for_pages(runtime: &mut StreamRuntime, pages: &[PageKey], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        runtime.pump_ready(8);
        if runtime.pin_pages(pages) {
            runtime.unpin_all();
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn split_runtime_preserves_pairing_and_rejects_cross_runtime_shutdown_without_mutation() {
    let mut first =
        StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).expect("first runtime");
    let mut second =
        StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).expect("second runtime");
    first
        .wait_until_ready(Duration::from_secs(2))
        .expect("first catalog ready");
    second
        .wait_until_ready(Duration::from_secs(2))
        .expect("second catalog ready");
    let (first_control, first_pages) = first.split().ok().expect("first split");
    let (second_control, second_pages) = second.split().ok().expect("second split");
    assert!(first_control.catalog_ready());
    assert!(second_control.catalog_ready());

    let mismatch = first_control
        .shutdown(second_pages)
        .expect_err("cross-runtime shutdown must be rejected");
    let (first_control, second_pages) = mismatch.into_parts().expect("recover mismatched pair");
    assert!(first_control.catalog_ready());
    assert_eq!(second_pages.stats(), Default::default());

    let first_report = first_control
        .shutdown(first_pages)
        .expect("matching first pair shuts down");
    let second_report = second_control
        .shutdown(second_pages)
        .expect("matching second pair shuts down");
    for report in [first_report, second_report] {
        assert!(report.worker_joined);
        assert!(report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert_eq!(report.worker_pages_reclaimed, 4);
    }
}

#[test]
fn dropping_control_keeps_real_worker_available_until_page_and_lease_release() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(expected.len(), 48_000);
    let key = StreamSourceKey::new(37, asset);
    let verified =
        VerifiedWavReader::open(wav.path(), reader_limits(expected.len())).expect("verified wav");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        expected.len() as u64,
        expected.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("metadata");
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata.clone(),
        reader_limits(expected.len()),
    );
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(2))
        .expect("catalog ready");
    let (control, mut pages) = runtime.split().ok().expect("split");
    let lease = control.catalog_lease().expect("verified catalog lease");
    assert!(lease.covers_source(&metadata));
    drop(control);

    pages.request_page(key, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ready = false;
    while Instant::now() < deadline {
        pages.pump_ready(3);
        if pages.page(key, 0).is_some() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        ready,
        "the worker remains available after control is dropped"
    );
    {
        let page = pages.page(key, 0).expect("requested page");
        assert_eq!(page.frames[0], expected[0]);
    }
    drop(pages);
    assert!(lease.covers_source(&metadata));
    drop(lease);
}

#[test]
fn catalog_lease_requires_readiness_and_matches_full_metadata_and_catalog_identity() {
    let (first_wav, _) = TestWav::stereo_pcm16(48_000, 4096);
    let (second_wav, _) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(4096, 48_000);
    let key = StreamSourceKey::new(41, asset);
    let verified =
        VerifiedWavReader::open(first_wav.path(), reader_limits(4096)).expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("source metadata");
    let first = SourceDefinition::new(
        first_wav.path().to_path_buf(),
        metadata.clone(),
        reader_limits(4096),
    );
    let second = SourceDefinition::new(
        second_wav.path().to_path_buf(),
        metadata.clone(),
        reader_limits(4096),
    );
    let first_runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![first]).expect("first runtime");
    let second_runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![second]).expect("second runtime");
    let (mut first_control, first_pages) = first_runtime.split().ok().expect("first split");
    let (mut second_control, second_pages) = second_runtime.split().ok().expect("second split");
    assert!(matches!(
        first_control.catalog_lease(),
        Err(hex_stream::StreamRuntimeError::CatalogNotReady)
    ));
    first_control
        .wait_until_ready(Duration::from_secs(2))
        .expect("first catalog ready");
    second_control
        .wait_until_ready(Duration::from_secs(2))
        .expect("second catalog ready");
    let first_lease = first_control.catalog_lease().expect("first lease");
    let second_lease = second_control.catalog_lease().expect("second lease");
    assert!(first_lease.covers_source(&metadata));
    assert!(second_lease.covers_source(&metadata));
    assert!(!first_lease.same_catalog(&second_lease));
    assert_eq!(first_lease.footprint().sources, 1);

    let wrong_fingerprint = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        [0x99; 32],
    )
    .unwrap();
    let wrong_generation = StreamedSourceMetadata::new(
        StreamSourceKey::new(42, asset),
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    let wrong_rate = StreamedSourceMetadata::new(
        key,
        44_100,
        48_000,
        4096,
        4458,
        hex_project::SINC_CONVERSION_V1,
        *verified.fingerprint(),
    )
    .unwrap();
    let wrong_frames = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4095,
        4095,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    for candidate in [
        wrong_fingerprint,
        wrong_generation,
        wrong_rate,
        wrong_frames,
    ] {
        assert!(!first_lease.covers_source(&candidate));
    }

    drop(first_lease);
    drop(second_lease);
    first_control.shutdown(first_pages).expect("first shutdown");
    second_control
        .shutdown(second_pages)
        .expect("second shutdown");
}

#[test]
fn busy_shutdown_recovers_unchanged_pair_until_all_catalog_leases_are_released() {
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(2))
        .expect("catalog ready");
    let (control, pages) = runtime.split().ok().expect("split");
    let lease = control.catalog_lease().expect("verified lease");
    let second_lease = lease.clone();
    let stats = pages.stats();
    let busy = control
        .shutdown(pages)
        .expect_err("outstanding graph leases must retain the worker");
    let (control, pages) = match busy {
        hex_stream::ShutdownError::Busy(busy) => {
            assert_eq!(busy.external_references(), 2);
            busy.into_parts()
        }
        other => panic!("expected busy shutdown, got {other:?}"),
    };
    assert!(control.catalog_ready());
    assert_eq!(pages.stats(), stats);
    drop(second_lease);
    drop(lease);
    let report = control
        .shutdown(pages)
        .expect("shutdown after lease release");
    assert!(report.worker_joined);
    assert!(report.accounting_complete);
}

#[test]
fn busy_shutdown_preserves_a_warmed_cache_for_retry() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(expected.len(), 48_000);
    let key = StreamSourceKey::new(43, asset);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(expected.len()))
        .expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        expected.len() as u64,
        expected.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("metadata");
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata,
        reader_limits(expected.len()),
    );
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(4), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(2))
        .expect("catalog ready");
    let (control, mut pages) = runtime.split().ok().expect("split");
    pages.request_page(key, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ready = false;
    while Instant::now() < deadline {
        pages.pump_ready(4);
        if pages.page(key, 0).is_some() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(ready, "source page is resident before attempted shutdown");
    let lease = control.catalog_lease().expect("catalog lease");
    let stats = pages.stats();
    let busy = control
        .shutdown(pages)
        .expect_err("shutdown must preserve a leased warm endpoint");
    let (control, mut pages) = match busy {
        hex_stream::ShutdownError::Busy(busy) => busy.into_parts(),
        other => panic!("expected busy shutdown, got {other:?}"),
    };
    assert_eq!(pages.stats(), stats);
    {
        let resident = pages.page(key, 0).expect("warmed page survived Busy");
        assert_eq!(resident.frames[0], expected[0]);
    }
    drop(lease);
    let report = control
        .shutdown(pages)
        .expect("shutdown after lease release");
    assert!(report.worker_joined);
    assert!(report.accounting_complete);
    assert_eq!(report.cached_pages_returned, 1);
}

#[test]
fn actual_worker_converts_and_recovers_after_a_full_unpinned_cache() {
    let (wav, source_pcm) = TestWav::stereo_pcm16(44_100, 12_001);
    let project_rate = 48_000;
    let asset = asset_id(source_pcm.len(), project_rate);
    let key = StreamSourceKey::new(17, asset);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(source_pcm.len()))
        .expect("verified source");
    let playback_frames = ((source_pcm.len() as u128 * u128::from(project_rate) + 44_100_u128 / 2)
        / 44_100_u128) as u64;
    let metadata = StreamedSourceMetadata::new(
        key,
        44_100,
        project_rate,
        source_pcm.len() as u64,
        playback_frames,
        hex_project::SINC_CONVERSION_V1,
        *verified.fingerprint(),
    )
    .expect("metadata");
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata.clone(),
        reader_limits(source_pcm.len()),
    );
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("catalog ready");
    let footprint = runtime.catalog_footprint().expect("catalog footprint");
    assert_eq!(footprint.sources, 1);
    assert_eq!(
        footprint.verified_reader_scratch_bytes,
        hex_media::VERIFIED_WAV_PAGE_BYTES
    );
    assert!(
        footprint.kernel_peak_bound_bytes
            <= StreamConfig::with_pool_pages(3).max_kernel_table_bytes
    );

    // Two cache slots fill the usable cache. A third request must make progress
    // using the separately reserved staging buffer and return an unpinned victim.
    let first = PageKey {
        source: key,
        page_index: 0,
    };
    let second = PageKey {
        source: key,
        page_index: 1,
    };
    let third = PageKey {
        source: key,
        page_index: 2,
    };
    for page in [first, second] {
        runtime.request_page(page.source, page.page_index);
    }
    assert!(wait_for_pages(
        &mut runtime,
        &[first, second],
        Duration::from_secs(3)
    ));
    runtime.request_page(third.source, third.page_index);
    assert!(wait_for_pages(
        &mut runtime,
        &[third],
        Duration::from_secs(3)
    ));
    let fourth = PageKey {
        source: key,
        page_index: 3,
    };
    runtime.request_page(fourth.source, fourth.page_index);
    assert!(wait_for_pages(
        &mut runtime,
        &[third, fourth],
        Duration::from_secs(3)
    ));

    // Exercise the same engine renderer with the warmed worker cache. Its
    // independent resident oracle uses the existing v1 conversion path.
    let original = Audio::new(44_100, source_pcm).expect("resident source");
    let imported = resample(
        &original,
        project_rate,
        ResampleLimits {
            max_source_frames: 100_000,
            max_output_frames: 100_000,
        },
    )
    .expect("resident oracle conversion");
    let source_start = 2 * hex_engine::STREAM_PAGE_FRAMES as u64;
    let expected = imported.frames()[source_start as usize..].to_vec();
    let clip = Clip::streamed_region(
        std::sync::Arc::new(metadata),
        0,
        expected.len() as u64,
        source_start,
        expected.len() as u64,
        false,
    )
    .expect("streamed clip");
    let project = EngineProject::new(
        project_rate,
        vec![Track::new(vec![clip], 1.0, false).expect("track")],
    )
    .expect("engine project");
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut output = vec![[0.0; 2]; expected.len()];
    let mut offset = 0;
    for block in [137, 503, 4093, 17, 1021, 3001, 5000, 7777] {
        if offset == output.len() {
            break;
        }
        let end = (offset + block).min(output.len());
        renderer
            .render_with_pages(&mut output[offset..end], &mut runtime)
            .expect("render warmed worker pages");
        offset = end;
    }
    if offset < output.len() {
        renderer
            .render_with_pages(&mut output[offset..], &mut runtime)
            .expect("render remainder");
    }
    assert_eq!(output, expected);
    assert_eq!(runtime.stats().misses, 0);
    assert_eq!(runtime.stats().queued_requests, 4);
    assert_eq!(runtime.stats().rejected_requests, 0);

    let report = runtime.shutdown().expect("balanced page-pool shutdown");
    assert!(report.worker_joined);
    assert!(report.accounting_complete);
    assert_eq!(report.page_buffers, 3);
    assert_eq!(
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed,
        report.page_buffers,
    );
}

#[test]
fn wrong_generation_is_rejected_and_mutated_sources_fail_without_publishing_a_page() {
    let (wav, _) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(4096, 48_000);
    let key = StreamSourceKey::new(31, asset);
    let verified =
        VerifiedWavReader::open(wav.path(), reader_limits(4096)).expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("identity metadata");
    let definition = SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(4096));
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("catalog ready");

    runtime.request_page(StreamSourceKey::new(32, asset), 0);
    assert_eq!(runtime.stats().rejected_requests, 1);

    let mut file = OpenOptions::new()
        .write(true)
        .open(wav.path())
        .expect("open mutation target");
    file.seek(SeekFrom::Start(44)).expect("seek to pcm");
    file.write_all(&[0x55, 0x33]).expect("mutate source");
    file.sync_all().expect("sync mutation");
    runtime.request_page(key, 0);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut failures = [hex_stream::StreamFailure {
        key,
        page_index: 0,
        kind: hex_stream::StreamFailureKind::DecodeFailed,
    }; 1];
    let mut failure_count = 0;
    while Instant::now() < deadline && failure_count == 0 {
        runtime.pump_ready(8);
        failure_count = runtime.take_failures(&mut failures);
        if failure_count == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(failure_count, 1);
    assert_eq!(failures[0].key, key);
    assert_eq!(failures[0].page_index, 0);
    assert_eq!(
        failures[0].kind,
        hex_stream::StreamFailureKind::SourceChanged
    );
    assert!(!runtime.pin_pages(&[PageKey {
        source: key,
        page_index: 0
    }]));

    let report = runtime.shutdown().expect("shutdown after source refusal");
    assert!(report.accounting_complete);
}

#[test]
fn catalog_rejects_nonfinite_float_samples_outside_the_first_page() {
    let wav = TestWav::stereo_float_with_nan(8192);
    let asset = asset_id(8192, 48_000);
    let key = StreamSourceKey::new(88, asset);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(8192))
        .expect("raw float bytes fingerprinted");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        8192,
        8192,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("float metadata");
    let definition = SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(8192));
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    assert_eq!(
        runtime.wait_until_ready(Duration::from_secs(3)),
        Err(hex_stream::StreamRuntimeError::CatalogFailed(
            hex_stream::StreamFailureKind::DecodeFailed,
        )),
    );
    let report = runtime.shutdown().expect("shutdown after float rejection");
    assert!(report.accounting_complete);
}

#[test]
fn shutdown_reclaims_undrained_ready_queue_and_worker_held_pages() {
    let (wav, samples) = TestWav::stereo_pcm16(48_000, 12_001);
    let asset = asset_id(samples.len(), 48_000);
    let key = StreamSourceKey::new(104, asset);
    let verified =
        VerifiedWavReader::open(wav.path(), reader_limits(samples.len())).expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        samples.len() as u64,
        samples.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .expect("identity metadata");
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata,
        reader_limits(samples.len()),
    );
    let mut config = StreamConfig::with_pool_pages(8);
    config.request_capacity = 8;
    config.ready_capacity = 1;
    config.max_ready_pages_per_pump = 1;
    config.return_capacity = 1;
    let mut runtime = StreamRuntime::new(config, vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("catalog ready");
    for page_index in 0..3 {
        runtime.request_page(key, page_index);
    }
    runtime.wake_worker();
    thread::sleep(Duration::from_millis(30));

    let report = runtime.shutdown().expect("shutdown with ready queue full");
    assert!(report.worker_joined);
    assert!(report.accounting_complete);
    assert_eq!(report.queued_pages_reclaimed, 1);
    assert_eq!(
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed,
        8,
    );
}

#[test]
fn complete_graph_binding_rejects_metadata_mismatches_and_recovers_both_owners() {
    let (wav, _) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(4096, 48_000);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(4096)).unwrap();
    let metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(41, asset),
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    let runtime = StreamRuntime::new(
        StreamConfig::with_pool_pages(3),
        vec![SourceDefinition::new(
            wav.path().to_path_buf(),
            metadata.clone(),
            reader_limits(4096),
        )],
    )
    .unwrap();
    let (mut control, pages) = runtime.split().ok().unwrap();
    control.wait_until_ready(Duration::from_secs(2)).unwrap();
    let variants = [
        StreamedSourceMetadata::new(
            StreamSourceKey::new(42, asset),
            48_000,
            48_000,
            4096,
            4096,
            hex_project::IDENTITY_CONVERSION,
            metadata.fingerprint(),
        )
        .unwrap(),
        StreamedSourceMetadata::new(
            metadata.key(),
            48_000,
            48_000,
            4096,
            4096,
            hex_project::IDENTITY_CONVERSION,
            [0xff; 32],
        )
        .unwrap(),
        StreamedSourceMetadata::new(
            metadata.key(),
            44_100,
            48_000,
            4096,
            4458,
            hex_project::SINC_CONVERSION_V1,
            metadata.fingerprint(),
        )
        .unwrap(),
        StreamedSourceMetadata::new(
            metadata.key(),
            44_100,
            44_100,
            4096,
            4096,
            hex_project::IDENTITY_CONVERSION,
            metadata.fingerprint(),
        )
        .unwrap(),
        StreamedSourceMetadata::new(
            metadata.key(),
            48_000,
            48_000,
            4095,
            4095,
            hex_project::IDENTITY_CONVERSION,
            metadata.fingerprint(),
        )
        .unwrap(),
    ];
    // An inconsistent conversion is rejected by the existing immutable metadata
    // constructor before it can enter any prepared graph or wrapper.
    assert!(
        StreamedSourceMetadata::new(
            metadata.key(),
            48_000,
            48_000,
            4096,
            4096,
            hex_project::SINC_CONVERSION_V1,
            metadata.fingerprint()
        )
        .is_err()
    );
    for variant in variants {
        for session_only in [false, true] {
            let graph = revision_for_stream(&variant, session_only);
            let lease = control.catalog_lease().unwrap();
            let error = hex_stream::VerifiedStreamRevision::new(Arc::clone(&graph), lease)
                .expect_err("complete graph must reject a mismatched binding");
            assert_eq!(error.source(), variant.key());
            let (returned, lease) = error.into_parts();
            assert!(Arc::ptr_eq(&graph, &returned));
            assert!(lease.covers_source(&metadata));
            assert!(control.catalog_ready());
            drop((returned, lease));
        }
    }
    let graph = revision_for_stream(&metadata, false);
    let old = hex_stream::VerifiedStreamRevision::new(
        Arc::clone(&graph),
        control.catalog_lease().unwrap(),
    )
    .unwrap();
    let candidate = hex_stream::VerifiedStreamRevision::new(
        revision_for_stream(&metadata, true),
        control.catalog_lease().unwrap(),
    )
    .unwrap();
    assert!(old.same_catalog(&candidate));
    assert!(!Arc::ptr_eq(old.revision(), candidate.revision()));
    assert_eq!(old.revision().streamed_sources().count(), 2);
    let stats = pages.stats();
    let error = control
        .shutdown(pages)
        .expect_err("old/candidate graphs retain real catalog owner");
    let (control, pages) = match error {
        hex_stream::ShutdownError::Busy(busy) => {
            assert_eq!(busy.external_references(), 2);
            busy.into_parts()
        }
        other => panic!("expected Busy, got {other:?}"),
    };
    assert_eq!(pages.stats(), stats);
    assert!(control.catalog_ready());
    assert!(old.catalog().covers_source(&metadata));
    drop(old);
    drop(candidate);
    let report = control.shutdown(pages).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn graph_wrappers_distinguish_real_catalogs_with_identical_source_keys() {
    let (wav, _) = TestWav::stereo_pcm16(48_000, 4096);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(4096)).unwrap();
    let metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(41, asset_id(4096, 48_000)),
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    let mut pairs = Vec::new();
    let mut graphs = Vec::new();
    let graph = revision_for_stream(&metadata, true);
    for _ in 0..2 {
        let runtime = StreamRuntime::new(
            StreamConfig::with_pool_pages(3),
            vec![SourceDefinition::new(
                wav.path().to_path_buf(),
                metadata.clone(),
                reader_limits(4096),
            )],
        )
        .unwrap();
        let (mut control, pages) = runtime.split().ok().unwrap();
        control.wait_until_ready(Duration::from_secs(2)).unwrap();
        graphs.push(
            hex_stream::VerifiedStreamRevision::new(
                Arc::clone(&graph),
                control.catalog_lease().unwrap(),
            )
            .unwrap(),
        );
        pairs.push((control, pages));
    }
    assert!(!graphs[0].same_catalog(&graphs[1]));
    assert!(Arc::ptr_eq(graphs[0].revision(), graphs[1].revision()));
    drop(graphs);
    for (control, pages) in pairs {
        let report = control.shutdown(pages).unwrap();
        assert!(report.worker_joined && report.accounting_complete);
    }
}

#[test]
fn streamed_binding_enumeration_covers_mixed_planes_without_cloning_metadata() {
    use hex_engine::compile::PreparedAudioSource;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let mut bank = BTreeMap::new();
    let mut tracks = Vec::new();
    let mut streamed = Vec::new();
    for index in 0..3 {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("track{index}"),
                path: format!("source{index}.wav").into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("import result");
        };
        tracks.push((track, asset));
        let source = if index < 2 {
            let metadata = Arc::new(
                StreamedSourceMetadata::new(
                    StreamSourceKey::new(51, asset),
                    48_000,
                    48_000,
                    4,
                    4,
                    hex_project::IDENTITY_CONVERSION,
                    [index as u8; 32],
                )
                .unwrap(),
            );
            streamed.push(Arc::clone(&metadata));
            PreparedAudioSource::Streamed(metadata)
        } else {
            PreparedAudioSource::Resident(Audio::new(48_000, vec![[0.25; 2]; 4]).unwrap())
        };
        bank.insert(asset, source);
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene result");
    };
    // Inventory order is independent of the Arrangement walk. Include resident
    // Session audio too, so neither plane may treat every audio clip as streamed.
    for &(track, asset) in tracks.iter().rev() {
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "one shot".into(),
                asset,
                source_start: 0,
                source_end: 4,
            })
            .unwrap();
    }
    let revision = hex_engine::PreparedRevision::prepare_with_prepared_audio_sources(
        editor.project(),
        &bank,
        51,
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    let owners: Vec<_> = streamed.iter().map(Arc::strong_count).collect();
    let actual: Vec<_> = revision.streamed_sources().collect();
    assert_eq!(actual.len(), 4);
    assert!(std::ptr::eq(actual[0], streamed[0].as_ref()));
    assert!(std::ptr::eq(actual[1], streamed[1].as_ref()));
    let expected_session: Vec<_> = revision
        .session()
        .audio_clips()
        .iter()
        .filter_map(|clip| match clip.source() {
            PreparedAudioSource::Streamed(source) => Some(source.as_ref()),
            PreparedAudioSource::Resident(_) => None,
        })
        .collect();
    assert_eq!(&actual[2..], expected_session.as_slice());
    assert_eq!(
        owners,
        streamed.iter().map(Arc::strong_count).collect::<Vec<_>>()
    );
    let facts: Vec<_> = actual.iter().map(|source| (*source).clone()).collect();
    drop(bank);
    drop(streamed);
    // These are borrowed immutable metadata, still owned by the exact revision
    // after the preparation bank and its external source handles are gone.
    assert_eq!(
        actual
            .iter()
            .map(|source| (*source).clone())
            .collect::<Vec<_>>(),
        facts
    );
}

#[test]
fn verified_page_endpoint_preserves_identical_key_wrong_owners_and_borrowed_pcm() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let key = StreamSourceKey::new(91, asset_id(4096, 48_000));
    let reader = VerifiedWavReader::open(wav.path(), reader_limits(4096)).unwrap();
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let mut pairs = Vec::new();
    for _ in 0..2 {
        let mut runtime = StreamRuntime::new(
            StreamConfig::with_pool_pages(3),
            vec![SourceDefinition::new(
                wav.path().to_path_buf(),
                metadata.clone(),
                reader_limits(4096),
            )],
        )
        .unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        pairs.push(runtime.split().ok().unwrap());
    }
    let (first_control, first_pages) = pairs.remove(0);
    let (second_control, second_pages) = pairs.remove(0);
    let first_lease = first_control.catalog_lease().unwrap();
    let second_lease = second_control.catalog_lease().unwrap();
    let error = hex_stream::VerifiedPageEndpoint::new(first_pages, second_lease)
        .err()
        .unwrap();
    let (first_pages, second_lease) = error.into_parts();
    assert_eq!(first_pages.stats(), Default::default());
    assert!(first_lease.covers_source(&metadata));
    assert!(second_lease.covers_source(&metadata));
    assert!(!first_lease.same_catalog(&second_lease));
    let extra_lease = first_lease.clone(); // control-only extra owner
    let mut endpoint = hex_stream::VerifiedPageEndpoint::new(first_pages, first_lease).unwrap();
    endpoint.pages().request_page(key, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        endpoint.pages().pump_ready(3);
        if let Some(page) = endpoint.pages().page(key, 0) {
            assert_eq!(page.frames, expected.as_slice());
            break;
        }
        assert!(Instant::now() < deadline, "real worker page ready");
        thread::sleep(Duration::from_millis(2));
    }
    let stats = endpoint.pages().stats();
    let (pages, lease) = endpoint.into_parts();
    let error = first_control.shutdown(pages).unwrap_err();
    let (first_control, pages) = error.into_parts().unwrap();
    assert_eq!(pages.stats(), stats);
    assert!(first_control.catalog_ready());
    drop(lease);
    let error = first_control.shutdown(pages).unwrap_err();
    let (first_control, mut pages) = error.into_parts().unwrap();
    assert_eq!(pages.page(key, 0).unwrap().frames, expected.as_slice());
    drop(extra_lease);
    thread::Builder::new()
        .name("hex-stream-endpoint-reclaim-test".into())
        .spawn(move || {
            let report = first_control.shutdown(pages).unwrap();
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 3);
            assert_eq!(
                report.worker_pages_reclaimed
                    + report.queued_pages_reclaimed
                    + report.held_return_pages_reclaimed,
                3
            );
        })
        .unwrap()
        .join()
        .unwrap();
    let endpoint = hex_stream::VerifiedPageEndpoint::new(second_pages, second_lease).unwrap();
    let (pages, lease) = endpoint.into_parts();
    drop(lease);
    let report = second_control.shutdown(pages).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn capacity_report_counts_reserved_storage_shared_kernels_and_pool_once() {
    let (wav, _) = TestWav::stereo_pcm16(44_100, 4096);
    let limits = reader_limits(4096);
    let reader = VerifiedWavReader::open(wav.path(), limits).unwrap();
    let converted_frames = hex_project::converted_frame_count(4096, 44_100, 48_000).unwrap();
    let mut definitions = Vec::with_capacity(7);
    let mut definition_paths = 0;
    for generation in [81, 82] {
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(generation, asset_id(4096, 48_000)),
            44_100,
            48_000,
            4096,
            converted_frames,
            hex_project::SINC_CONVERSION_V1,
            *reader.fingerprint(),
        )
        .unwrap();
        let mut path = wav.path().to_path_buf();
        path.reserve(1024);
        definition_paths += path.capacity();
        definitions.push(SourceDefinition::new(path, metadata, limits));
    }
    let definition_capacity = definitions.capacity();
    let config = StreamConfig {
        source_scratch_frames: 8192,
        ..StreamConfig::with_pool_pages(4)
    };
    let mut runtime = StreamRuntime::new(config, definitions).unwrap();
    assert!(runtime.allocation_footprint().is_none());
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let report = runtime.allocation_footprint().unwrap();
    assert_eq!(
        report.construction.definition_slots.capacity,
        definition_capacity
    );
    assert_eq!(report.catalog.definition_path_bytes, definition_paths);
    assert_eq!(
        report.catalog.reader_path_bytes,
        2 * reader.path_capacity_bytes()
    );
    assert_eq!(
        report.catalog.manifest_bytes,
        2 * reader.page_manifest_bytes()
    );
    assert_eq!(report.catalog.membership_slots.capacity, 2);
    assert_eq!(report.catalog.source_slots.capacity, 2);
    // Reader scratch is inline in Source; no separate additive scratch charge.
    assert!(report.catalog.source_slots.element_bytes > hex_media::VERIFIED_WAV_PAGE_BYTES);
    let kernel = hex_media::ResampleKernel::new(44_100, 48_000).unwrap();
    assert_eq!(
        report.catalog.kernel_weight_bytes,
        kernel.table_payload_bytes()
    );
    assert_eq!(
        report.catalog.kernel_object_bytes,
        kernel.shared_object_bytes()
    );
    assert_eq!(
        report.construction.kernel_table_peak_bound_bytes,
        13 * 1024 * 1024
    );
    assert_eq!(report.transport.pcm_frames.bytes(), Some(4 * 4096 * 8));
    assert_eq!(report.transport.page_objects.capacity, 4);
    assert_eq!(report.transport.page_boxing_peak_extra.capacity, 4096);
    assert_eq!(report.transport.request_slots.capacity, 8);
    assert_eq!(report.transport.ready_slots.capacity, 4);
    assert_eq!(report.transport.return_slots.capacity, 4);
    assert_eq!(report.transport.failure_slots.capacity, 4);
    assert_eq!(report.endpoint.max_cached_pages, 3);
    assert_eq!(report.endpoint.pending.capacity, 16);
    assert_eq!(report.endpoint.cache.capacity, 8);
    assert_eq!(report.endpoint.held_returns.capacity, 5);
    assert_eq!(report.catalog.source_scratch.capacity, 8192);
    assert_eq!(
        report.retained_bytes().unwrap(),
        report.endpoint.bytes().unwrap()
            + report.transport.bytes().unwrap()
            + report.catalog.bytes().unwrap()
    );
    let (control, pages) = runtime.split().ok().unwrap();
    let extra = control.catalog_lease().unwrap();
    let mut endpoint =
        hex_stream::VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
    assert_eq!(endpoint.allocation_footprint(), report);
    let key = StreamSourceKey::new(81, asset_id(4096, 48_000));
    endpoint.pages().request_page(key, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        endpoint.pages().pump_ready(4);
        if endpoint.pages().page(key, 0).is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    // Moving a page through queues/cache and cloning a control lease changes
    // ownership counts, not allocation-capacity charges.
    assert_eq!(endpoint.allocation_footprint(), report);
    let (pages, lease) = endpoint.into_parts();
    drop(lease);
    drop(extra);
    assert!(control.shutdown(pages).unwrap().accounting_complete);
    println!("two-source one-kernel capacity report: {report:?}");
    println!(
        "retained={} construction_bound={}",
        report.retained_bytes().unwrap(),
        report.construction_peak_bound_bytes().unwrap()
    );
}
