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
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
