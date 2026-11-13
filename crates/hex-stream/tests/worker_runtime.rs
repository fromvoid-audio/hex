use hex_engine::{
    Audio, Clip, MediaPages, Project as EngineProject, Renderer, StreamSourceKey,
    StreamedSourceMetadata, Track,
};
use hex_media::{
    ResampleLimits, ResampleVersion, VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits,
    resample, resample_with_version,
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

#[cfg(feature = "test-support")]
fn wait_for_split_page(pages: &mut hex_stream::StreamCallbackPages, key: PageKey) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        pages.pump_ready(8);
        if pages.pin_pages(&[key]) {
            pages.unpin_all();
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

#[cfg(feature = "test-support")]
fn gated_second_page(release_page: bool) {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 8192);
    let asset = asset_id(expected.len(), 48_000);
    let key = StreamSourceKey::new(301, asset);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(expected.len())).unwrap();
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        expected.len() as u64,
        expected.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata,
        reader_limits(expected.len()),
    );
    let pages = [0, 1].map(|page_index| PageKey {
        source: key,
        page_index,
    });
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(pages[1]);
    let mut runtime =
        StreamRuntime::new_with_page_gate(StreamConfig::with_pool_pages(3), vec![definition], gate)
            .unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let (control, mut endpoint) = runtime.split().ok().unwrap();
    endpoint.request_page(key, 0);
    control.wake_worker();
    assert!(wait_for_split_page(&mut endpoint, pages[0]));
    assert_eq!(endpoint.page(key, 0).unwrap().frames, &expected[..4096]);
    endpoint.request_page(key, 1);
    control.wake_worker();
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    endpoint.pump_ready(8);
    assert!(endpoint.page(key, 1).is_none());
    assert_eq!(endpoint.page(key, 0).unwrap().frames, &expected[..4096]);
    if release_page {
        release.send(()).unwrap();
        assert!(wait_for_split_page(&mut endpoint, pages[1]));
        assert_eq!(endpoint.page(key, 1).unwrap().frames, &expected[4096..]);
    }
    let report = control.shutdown(endpoint).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 3);
}

#[cfg(feature = "test-support")]
#[test]
fn held_later_page_releases_after_initial_ready() {
    gated_second_page(true);
}

#[cfg(feature = "test-support")]
#[test]
fn shutdown_joins_worker_with_later_page_held() {
    gated_second_page(false);
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
fn pinned_real_worker_pages_survive_pressure_and_retry_after_unpin() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 12_288);
    let asset = asset_id(expected.len(), 48_000);
    let key = StreamSourceKey::new(117, asset);
    let verified = VerifiedWavReader::open(wav.path(), reader_limits(expected.len())).unwrap();
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        expected.len() as u64,
        expected.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata,
        reader_limits(expected.len()),
    );
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let pages = [0, 1, 2].map(|page_index| PageKey {
        source: key,
        page_index,
    });
    for page in &pages[..2] {
        runtime.request_page(key, page.page_index);
    }
    assert!(wait_for_pages(
        &mut runtime,
        &pages[..2],
        Duration::from_secs(3)
    ));
    assert!(runtime.pin_pages(&pages[..2]));
    let queued = runtime.stats().queued_requests;
    runtime.request_page(key, 2);
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime.stats().queued_requests < queued + 2 {
        assert!(
            Instant::now() < deadline,
            "refused page did not become retryable"
        );
        runtime.pump_ready(8);
        assert!(runtime.page(key, 2).is_none());
        runtime.request_page(key, 2);
        thread::sleep(Duration::from_millis(2));
    }
    for (index, page) in pages[..2].iter().enumerate() {
        assert_eq!(
            runtime.page(key, page.page_index).unwrap().frames,
            &expected[index * 4096..(index + 1) * 4096]
        );
    }
    runtime.unpin_all();
    assert!(wait_for_pages(
        &mut runtime,
        &pages[2..],
        Duration::from_secs(3)
    ));
    assert_eq!(runtime.page(key, 2).unwrap().frames, &expected[8192..]);
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 3);
}

#[test]
fn identity_pages_preserve_exact_samples_and_partial_eof_after_cache_turnover() {
    let page_frames = hex_engine::STREAM_PAGE_FRAMES;
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, page_frames * 2 + 17);
    let key = StreamSourceKey::new(43, asset_id(pcm.len(), 48_000));
    let reader =
        VerifiedWavReader::open(wav.path(), reader_limits(pcm.len())).expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        pcm.len() as u64,
        pcm.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .expect("identity metadata");
    let definition =
        SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(pcm.len()));
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("ready");

    // Turn over the two cache slots and compare each exposed page directly
    // with the fixture's decoded PCM, including the short final page.
    for index in [0, 1, 2, 0, 2] {
        let page = PageKey {
            source: key,
            page_index: index,
        };
        if runtime.page(key, index).is_none() {
            runtime.request_page(key, index);
        }
        assert!(wait_for_pages(
            &mut runtime,
            &[page],
            Duration::from_secs(3)
        ));
        let actual = runtime.page(key, index).expect("installed page");
        let start = index as usize * page_frames;
        let end = (start + page_frames).min(pcm.len());
        assert_eq!(actual.page_index, index);
        assert_eq!(actual.frames, &pcm[start..end]);
    }

    let report = runtime.shutdown().expect("balanced shutdown");
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 3);
}

#[test]
fn real_worker_keeps_simultaneous_generations_of_an_asset_separate() {
    let frames = hex_engine::STREAM_PAGE_FRAMES;
    let (first_wav, first_pcm) = TestWav::stereo_pcm16(48_000, frames);
    let (second_wav, mut second_pcm) = TestWav::stereo_pcm16(48_000, frames);
    let mut file = OpenOptions::new()
        .write(true)
        .open(second_wav.path())
        .unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    file.write_all(&16_384_i16.to_le_bytes()).unwrap();
    file.sync_all().unwrap();
    second_pcm[0][0] = 0.5;
    assert_ne!(first_pcm, second_pcm);

    let asset = asset_id(frames, 48_000);
    let keys = [
        StreamSourceKey::new(41, asset),
        StreamSourceKey::new(42, asset),
    ];
    let definitions = [&first_wav, &second_wav]
        .into_iter()
        .zip(keys)
        .map(|(wav, key)| {
            let reader = VerifiedWavReader::open(wav.path(), reader_limits(frames)).unwrap();
            let metadata = StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                frames as u64,
                frames as u64,
                hex_project::IDENTITY_CONVERSION,
                *reader.fingerprint(),
            )
            .unwrap();
            SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(frames))
        })
        .collect();
    let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(3), definitions).unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let pages = keys.map(|source| PageKey {
        source,
        page_index: 0,
    });
    for key in keys {
        runtime.request_page(key, 0);
    }
    assert!(wait_for_pages(&mut runtime, &pages, Duration::from_secs(3)));
    assert!(runtime.pin_pages(&pages));
    assert_eq!(runtime.page(keys[0], 0).unwrap().frames, first_pcm);
    assert_eq!(runtime.page(keys[1], 0).unwrap().frames, second_pcm);
    runtime.unpin_all();
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 3);
}

#[test]
fn wrong_generation_is_rejected_and_mutated_sources_fail_without_publishing_a_page() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let original = fs::read(wav.path()).expect("read original fixture");
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
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition.clone()])
            .expect("runtime");
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
        key: key.into(),
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
    assert_eq!(failures[0].key, key.into());
    assert_eq!(failures[0].page_index, 0);
    assert_eq!(
        failures[0].kind,
        hex_stream::StreamFailureKind::SourceChanged
    );
    assert!(!runtime.pin_pages(&[PageKey {
        source: key,
        page_index: 0
    }]));

    fs::write(wav.path(), &original).expect("restore exact original bytes");
    OpenOptions::new()
        .write(true)
        .open(wav.path())
        .expect("open restored fixture")
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
        .expect("set a different modification time");
    let page = PageKey {
        source: key,
        page_index: 0,
    };
    runtime.request_page(key, 0);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut failure_count = 0;
    while Instant::now() < deadline && failure_count == 0 {
        runtime.pump_ready(8);
        failure_count = runtime.take_failures(&mut failures);
        if failure_count == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(failure_count, 1);
    assert_eq!(failures[0].key, key.into());
    assert_eq!(failures[0].page_index, 0);
    assert_eq!(
        failures[0].kind,
        hex_stream::StreamFailureKind::SourceChanged
    );
    assert!(runtime.page(key, 0).is_none());
    let report = runtime.shutdown().expect("shutdown after source refusal");
    assert!(report.worker_joined && report.accounting_complete);

    let mut recovered = StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition])
        .expect("fresh runtime");
    recovered
        .wait_until_ready(Duration::from_secs(3))
        .expect("reverified original source");
    recovered.request_page(key, 0);
    assert!(wait_for_pages(
        &mut recovered,
        &[page],
        Duration::from_secs(3)
    ));
    assert_eq!(
        recovered.page(key, 0).expect("restored page").frames,
        expected
    );
    let report = recovered.shutdown().expect("shutdown after recovery");
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn real_worker_loop_crosses_page_boundaries_with_exact_pcm() {
    let page_frames = hex_engine::STREAM_PAGE_FRAMES;
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, page_frames + 16);
    let key = StreamSourceKey::new(44, asset_id(pcm.len(), 48_000));
    let reader =
        VerifiedWavReader::open(wav.path(), reader_limits(pcm.len())).expect("verified source");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        pcm.len() as u64,
        pcm.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .expect("identity metadata");
    let definition = SourceDefinition::new(
        wav.path().to_path_buf(),
        metadata.clone(),
        reader_limits(pcm.len()),
    );
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("ready");
    let pages = [
        PageKey {
            source: key,
            page_index: 0,
        },
        PageKey {
            source: key,
            page_index: 1,
        },
    ];
    for page in pages {
        runtime.request_page(page.source, page.page_index);
    }
    assert!(wait_for_pages(&mut runtime, &pages, Duration::from_secs(3)));

    let clip = Clip::streamed_region(
        std::sync::Arc::new(metadata),
        3,
        20,
        (page_frames - 3) as u64,
        9,
        true,
    )
    .expect("looped clip");
    let project = EngineProject::new(
        48_000,
        vec![Track::new(vec![clip], 1.0, false).expect("track")],
    )
    .expect("project");
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut output = [[0.0; 2]; 26];
    let mut offset = 0;
    for count in [2, 5, 1, 7, 3, 8] {
        renderer
            .render_with_pages(&mut output[offset..offset + count], &mut runtime)
            .expect("render");
        offset += count;
    }
    let mut expected = [[0.0; 2]; 26];
    for (frame, sample) in expected.iter_mut().enumerate().skip(3).take(20) {
        *sample = pcm[page_frames - 3 + (frame - 3) % 9];
    }
    assert_eq!(output, expected);
    assert_eq!(runtime.stats().misses, 0);
    let report = runtime.shutdown().expect("balanced shutdown");
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn real_worker_refuses_same_metadata_edit_without_losing_pinned_healthy_page() {
    let page_frames = hex_engine::STREAM_PAGE_FRAMES;
    let frames = page_frames * 8;
    let (damaged, original_pcm) = TestWav::stereo_pcm16(48_000, frames);
    let (healthy, healthy_pcm) = TestWav::stereo_pcm16(48_000, frames);
    let original = fs::read(damaged.path()).unwrap();
    let asset = asset_id(frames, 48_000);
    let damaged_key = StreamSourceKey::new(201, asset);
    let healthy_key = StreamSourceKey::new(202, asset);
    let definition = |wav: &TestWav, key| {
        let reader = VerifiedWavReader::open(wav.path(), reader_limits(frames)).unwrap();
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            frames as u64,
            frames as u64,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(frames))
    };
    let damaged_definition = definition(&damaged, damaged_key);
    let healthy_definition = definition(&healthy, healthy_key);
    let mut runtime = StreamRuntime::new(
        StreamConfig::with_pool_pages(3),
        vec![damaged_definition.clone(), healthy_definition],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let healthy_page = PageKey {
        source: healthy_key,
        page_index: 0,
    };
    runtime.request_page(healthy_key, 0);
    assert!(wait_for_pages(
        &mut runtime,
        &[healthy_page],
        Duration::from_secs(3)
    ));
    assert!(runtime.pin_pages(&[healthy_page]));
    assert_eq!(
        runtime.page(healthy_key, 0).unwrap().frames,
        &healthy_pcm[..page_frames]
    );

    let page_index = 5_u64;
    let frame = page_frames * page_index as usize;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(damaged.path())
        .unwrap();
    let before = file.metadata().unwrap();
    let modified = before.modified().unwrap();
    let offset = 44 + frame * 4;
    file.seek(SeekFrom::Start(offset as u64)).unwrap();
    file.write_all(&[original[offset] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let after = file.metadata().unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), modified);
    assert_ne!(fs::read(damaged.path()).unwrap(), original);

    let damaged_page = PageKey {
        source: damaged_key,
        page_index,
    };
    runtime.request_page(damaged_key, page_index);
    let mut failures = [hex_stream::StreamFailure {
        key: damaged_key.into(),
        page_index,
        kind: hex_stream::StreamFailureKind::DecodeFailed,
    }; 1];
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut count = 0;
    while Instant::now() < deadline && count == 0 {
        runtime.pump_ready(8);
        count = runtime.take_failures(&mut failures);
        if count == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(count, 1);
    assert_eq!(failures[0].key, damaged_key.into());
    assert_eq!(failures[0].page_index, page_index);
    assert_eq!(
        failures[0].kind,
        hex_stream::StreamFailureKind::SourceChanged
    );
    assert!(runtime.page(damaged_key, page_index).is_none());
    assert_eq!(
        runtime.page(healthy_key, 0).unwrap().frames,
        &healthy_pcm[..page_frames]
    );
    runtime.unpin_all();
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);

    fs::write(damaged.path(), &original).unwrap();
    assert_eq!(fs::read(damaged.path()).unwrap(), original);
    let mut recovered =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![damaged_definition]).unwrap();
    recovered.wait_until_ready(Duration::from_secs(3)).unwrap();
    recovered.request_page(damaged_key, page_index);
    assert!(wait_for_pages(
        &mut recovered,
        &[damaged_page],
        Duration::from_secs(3)
    ));
    assert_eq!(
        recovered.page(damaged_key, page_index).unwrap().frames,
        &original_pcm[frame..frame + page_frames]
    );
    let report = recovered.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn truncated_source_refuses_its_page_without_stalling_another_source() {
    let (damaged, _) = TestWav::stereo_pcm16(48_000, 4096);
    let (healthy, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(4096, 48_000);
    let damaged_key = StreamSourceKey::new(101, asset);
    let healthy_key = StreamSourceKey::new(102, asset);
    let definition = |wav: &TestWav, key| {
        let reader = VerifiedWavReader::open(wav.path(), reader_limits(4096)).expect("verified");
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4096,
            4096,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .expect("metadata");
        SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(4096))
    };
    let mut runtime = StreamRuntime::new(
        StreamConfig::with_pool_pages(3),
        vec![
            definition(&damaged, damaged_key),
            definition(&healthy, healthy_key),
        ],
    )
    .expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("ready");
    OpenOptions::new()
        .write(true)
        .open(damaged.path())
        .expect("open damaged fixture")
        .set_len(48)
        .expect("truncate fixture");
    runtime.request_page(damaged_key, 0);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut failures = [hex_stream::StreamFailure {
        key: damaged_key.into(),
        page_index: 0,
        kind: hex_stream::StreamFailureKind::DecodeFailed,
    }; 1];
    let mut count = 0;
    while Instant::now() < deadline && count == 0 {
        runtime.pump_ready(8);
        count = runtime.take_failures(&mut failures);
        if count == 0 {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(count, 1);
    assert_eq!(failures[0].key, damaged_key.into());
    assert_eq!(failures[0].page_index, 0);
    assert_eq!(
        failures[0].kind,
        hex_stream::StreamFailureKind::SourceChanged
    );
    assert!(runtime.page(damaged_key, 0).is_none());

    let page = PageKey {
        source: healthy_key,
        page_index: 0,
    };
    runtime.request_page(healthy_key, 0);
    assert!(wait_for_pages(
        &mut runtime,
        &[page],
        Duration::from_secs(3)
    ));
    assert_eq!(
        runtime.page(healthy_key, 0).expect("healthy page").frames,
        expected
    );
    let report = runtime.shutdown().expect("balanced shutdown");
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn deleted_source_keeps_serving_verified_pcm_and_a_new_catalog_refuses_it() {
    let (wav, expected) = TestWav::stereo_pcm16(48_000, 4096);
    let asset = asset_id(4096, 48_000);
    let key = StreamSourceKey::new(103, asset);
    let reader = VerifiedWavReader::open(wav.path(), reader_limits(4096)).expect("verified");
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        4096,
        4096,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .expect("metadata");
    drop(reader);
    let definition = || {
        SourceDefinition::new(
            wav.path().to_path_buf(),
            metadata.clone(),
            reader_limits(4096),
        )
    };
    let mut runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition()]).expect("runtime");
    runtime
        .wait_until_ready(Duration::from_secs(3))
        .expect("ready");
    // Removing the path after verification leaves the worker's open descriptor
    // on the same verified bytes; it must neither fail nor serve other data.
    assert!(
        runtime.page(key, 0).is_none(),
        "page must be read after deletion"
    );
    fs::remove_file(wav.path()).expect("delete fixture");
    let page = PageKey {
        source: key,
        page_index: 0,
    };
    runtime.request_page(key, 0);
    assert!(wait_for_pages(
        &mut runtime,
        &[page],
        Duration::from_secs(3)
    ));
    assert_eq!(runtime.page(key, 0).expect("page").frames, expected);
    let report = runtime.shutdown().expect("balanced shutdown");
    assert!(report.worker_joined && report.accounting_complete);

    // Reopening the deleted path is an explicit catalog failure, not a stall.
    let mut reopened =
        StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![definition()]).expect("runtime");
    assert!(reopened.wait_until_ready(Duration::from_secs(3)).is_err());
    let report = reopened.shutdown().expect("balanced shutdown");
    assert!(report.worker_joined && report.accounting_complete);
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

/// Streamed pages reproduce each asset's saved conversion law bit-for-bit,
/// including v2's wider downsampling halo; v1 output keeps the v1 law.
#[test]
fn actual_worker_renders_each_saved_conversion_law() {
    for (source_rate, project_rate) in [(44_100, 48_000), (96_000, 44_100)] {
        let (wav, source_pcm) = TestWav::stereo_pcm16(source_rate, 12_001);
        let original = Audio::new(source_rate, source_pcm.clone()).expect("resident source");
        let limits = ResampleLimits {
            max_source_frames: 100_000,
            max_output_frames: 100_000,
        };
        let mut laws = Vec::new();
        for (generation, conversion, version) in [
            (21, hex_project::SINC_CONVERSION_V1, ResampleVersion::V1),
            (22, hex_project::SINC_CONVERSION_V2, ResampleVersion::V2),
        ] {
            let verified = VerifiedWavReader::open(wav.path(), reader_limits(source_pcm.len()))
                .expect("verified source");
            let playback_frames = hex_project::converted_frame_count(
                source_pcm.len() as u64,
                source_rate,
                project_rate,
            )
            .expect("playback frames");
            let key = StreamSourceKey::new(generation, asset_id(source_pcm.len(), project_rate));
            let metadata = StreamedSourceMetadata::new(
                key,
                source_rate,
                project_rate,
                source_pcm.len() as u64,
                playback_frames,
                conversion,
                *verified.fingerprint(),
            )
            .expect("metadata");
            let definition = SourceDefinition::new(
                wav.path().to_path_buf(),
                metadata.clone(),
                reader_limits(source_pcm.len()),
            );
            let mut runtime =
                StreamRuntime::new(StreamConfig::with_pool_pages(8), vec![definition])
                    .expect("runtime");
            runtime
                .wait_until_ready(Duration::from_secs(3))
                .expect("catalog ready");
            let expected = resample_with_version(&original, project_rate, limits, version)
                .expect("resident oracle")
                .frames()
                .to_vec();
            let clip = Clip::streamed_region(
                std::sync::Arc::new(metadata),
                0,
                expected.len() as u64,
                0,
                expected.len() as u64,
                false,
            )
            .expect("streamed clip");
            let project = EngineProject::new(
                project_rate,
                vec![Track::new(vec![clip], 1.0, false).expect("track")],
            )
            .expect("engine project");
            let pages = expected.len().div_ceil(hex_engine::STREAM_PAGE_FRAMES);
            let keys: Vec<_> = (0..pages as u64)
                .map(|page_index| PageKey {
                    source: key,
                    page_index,
                })
                .collect();
            for page in &keys {
                runtime.request_page(page.source, page.page_index);
            }
            assert!(wait_for_pages(&mut runtime, &keys, Duration::from_secs(3)));
            let mut renderer = Renderer::new(&project);
            renderer.play();
            let mut output = vec![[0.0; 2]; expected.len()];
            renderer
                .render_with_pages(&mut output, &mut runtime)
                .expect("render pages");
            assert_eq!(
                output, expected,
                "{source_rate}->{project_rate} {version:?}"
            );
            laws.push(output);
            runtime.shutdown().expect("shutdown");
        }
        assert_ne!(laws[0], laws[1]);
    }
}

#[cfg(feature = "test-support")]
#[test]
fn real_worker_recovers_competing_demand_while_prefetch_is_in_flight() {
    let page_frames = hex_engine::STREAM_PAGE_FRAMES;
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, page_frames * 4);
    let key = StreamSourceKey::new(89, asset_id(pcm.len(), 48_000));
    let reader = VerifiedWavReader::open(wav.path(), reader_limits(pcm.len())).unwrap();
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        pcm.len() as u64,
        pcm.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let definition =
        SourceDefinition::new(wav.path().to_path_buf(), metadata, reader_limits(pcm.len()));
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(PageKey {
        source: key,
        page_index: 2,
    });
    let mut runtime =
        StreamRuntime::new_with_page_gate(StreamConfig::with_pool_pages(3), vec![definition], gate)
            .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let warm = [
        PageKey {
            source: key,
            page_index: 0,
        },
        PageKey {
            source: key,
            page_index: 1,
        },
    ];
    for page in warm {
        runtime.request_page(page.source, page.page_index);
    }
    assert!(wait_for_pages(&mut runtime, &warm, Duration::from_secs(2)));
    runtime.prefetch_page(key, 2);
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    runtime.request_page(key, 3);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut failures = [hex_stream::StreamFailure {
        key: key.into(),
        page_index: 0,
        kind: hex_stream::StreamFailureKind::DecodeFailed,
    }; 4];
    loop {
        runtime.pump_ready(3);
        if runtime.page(key, 3).is_some() {
            break;
        }
        let count = runtime.take_failures(&mut failures);
        for failure in &failures[..count] {
            assert_eq!(failure.key, key.into());
            assert_eq!(failure.page_index, 3);
            assert_eq!(
                failure.kind,
                hex_stream::StreamFailureKind::CapacityExhausted
            );
            runtime.request_page(key, 3);
        }
        assert!(
            Instant::now() < deadline,
            "competing demand did not recover"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let page = runtime.page(key, 3).unwrap();
    assert_eq!(page.frames, &pcm[page_frames * 3..]);
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn pitched_worker_pages_match_resident_for_raw_and_converted_sources() {
    use hex_project::WarpResampler;
    use hex_warp::{Input, MapSegment, Mode, Request, Resampler, TimeMap, render_with_resampler};
    for source_rate in [48_000, 44_100] {
        let project_rate = 48_000;
        let (wav, pcm) = TestWav::stereo_pcm16(source_rate, 12_001);
        let limits = reader_limits(pcm.len());
        let verified = VerifiedWavReader::open(wav.path(), limits).unwrap();
        let playback_frames =
            hex_project::converted_frame_count(pcm.len() as u64, source_rate, project_rate)
                .unwrap();
        let conversion = if source_rate == project_rate {
            hex_project::IDENTITY_CONVERSION
        } else {
            hex_project::SINC_CONVERSION_V2
        };
        let raw = StreamedSourceMetadata::new(
            StreamSourceKey::new(401, asset_id(pcm.len(), project_rate)),
            source_rate,
            project_rate,
            pcm.len() as u64,
            playback_frames,
            conversion,
            *verified.fingerprint(),
        )
        .unwrap();
        let up = raw.with_plain_pitch(700, WarpResampler::V2).unwrap();
        let down = raw.with_plain_pitch(-700, WarpResampler::V1).unwrap();
        assert!(up.with_plain_pitch(1, WarpResampler::V1).is_err());
        let definitions = [&raw, &up, &down]
            .into_iter()
            .map(|metadata| {
                SourceDefinition::new(wav.path().to_path_buf(), metadata.clone(), limits)
                    .with_verified_manifest(verified.manifest().clone())
            })
            .collect();
        let mut runtime =
            StreamRuntime::new(StreamConfig::with_pool_pages(12), definitions).unwrap();
        runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
        let footprint = runtime.catalog_footprint().unwrap();
        let stale =
            StreamSourceKey::plain_pitch(999, raw.key().asset(), 700, WarpResampler::V2).unwrap();
        let rejected_before = runtime.stats().rejected_requests;
        runtime.request_page(stale, 0);
        assert_eq!(runtime.stats().rejected_requests, rejected_before + 1);
        assert!(runtime.page(stale, 0).is_none());
        assert_eq!(
            footprint.pitch_scratch_bytes,
            66_048 * std::mem::size_of::<[f32; 2]>()
        );
        assert!(footprint.kernel_table_bytes > 128 * 1024);
        assert!(footprint.kernel_table_bytes <= footprint.kernel_peak_bound_bytes);
        let original = Audio::new(source_rate, pcm).unwrap();
        let project = if source_rate == project_rate {
            original
        } else {
            resample_with_version(
                &original,
                project_rate,
                ResampleLimits {
                    max_source_frames: 100_000,
                    max_output_frames: 100_000,
                },
                ResampleVersion::V2,
            )
            .unwrap()
        };
        let frames = project.frames();
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: playback_frames,
                source_start_q32: 0,
                source_end_q32: i128::from(playback_frames) << 32,
            }],
            playback_frames as usize,
        )
        .unwrap();
        for (metadata, cents, law) in [(&up, 700, Resampler::V2), (&down, -700, Resampler::V1)] {
            let expected = render_with_resampler(
                &Request {
                    attacks: &[],
                    input: Input {
                        frames,
                        sample_rate: project_rate,
                        region_start: 0,
                        region_length: playback_frames,
                        phase: 0,
                        looping: false,
                        envelope: &[],
                    },
                    map: &map,
                    output_frames: playback_frames as usize,
                    pitch_cents: cents,
                    mode: Mode::Resample,
                },
                law,
                &mut || false,
            )
            .unwrap()
            .frames;
            for index in [0, 1, (playback_frames - 1) / 4096, 1] {
                let key = PageKey {
                    source: metadata.key(),
                    page_index: index,
                };
                runtime.request_page(key.source, index);
                assert!(wait_for_pages(&mut runtime, &[key], Duration::from_secs(3)));
                let page = runtime.page(key.source, index).unwrap();
                let start = index as usize * 4096;
                assert_eq!(
                    page.frames,
                    &expected[start..start + page.frames.len()],
                    "{source_rate} hz, {cents} cents, page {index}"
                );
            }
        }
        runtime.request_page(raw.key(), 0);
        let found = wait_for_pages(
            &mut runtime,
            &[PageKey {
                source: raw.key(),
                page_index: 0,
            }],
            Duration::from_millis(400),
        );
        if !found {
            let mut failures = [hex_stream::StreamFailure {
                key: raw.key().into(),
                page_index: 0,
                kind: hex_stream::StreamFailureKind::Cancelled,
            }; 8];
            let count = runtime.take_failures(&mut failures);
            panic!(
                "raw source missing at {source_rate}: {:?}, failures {:?}",
                runtime.stats(),
                &failures[..count]
            );
        }
        assert_eq!(runtime.page(raw.key(), 0).unwrap().frames, &frames[..4096]);
        assert!(runtime.shutdown().unwrap().accounting_complete);
    }
}

#[test]
fn pitched_worker_keeps_twenty_million_frame_source_bounded() {
    const FRAMES: u64 = 20_000_000;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hex-stream-long-pitch-{}-{stamp}-{}.wav",
        std::process::id(),
        next_fixture_id()
    ));
    let wav = TestWav(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(wav.path())
        .unwrap();
    let bytes = (FRAMES * 4) as u32;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&48_000_u32.to_le_bytes()).unwrap();
    file.write_all(&192_000_u32.to_le_bytes()).unwrap();
    file.write_all(&4_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    file.set_len(44 + u64::from(bytes)).unwrap();
    let index = (FRAMES - 1) / 4096;
    let step_q32 = (2.0_f64.powf(-700.0 / 1_200.0) * (1_u64 << 32) as f64).round() as u64;
    let impulse_frame = ((u128::from(index * 4096) * u128::from(step_q32)) >> 32) as u64;
    file.seek(SeekFrom::Start(44 + impulse_frame * 4)).unwrap();
    file.write_all(&16_384_i16.to_le_bytes()).unwrap();
    file.write_all(&(-8_192_i16).to_le_bytes()).unwrap();
    drop(file);
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(100_000_000, FRAMES, 128, 32_768),
        2_000_000,
    );
    let reader = VerifiedWavReader::open(wav.path(), limits).unwrap();
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(402, asset_id(FRAMES as usize, 48_000)),
        48_000,
        48_000,
        FRAMES,
        FRAMES,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let metadata = raw
        .with_plain_pitch(-700, hex_project::WarpResampler::V2)
        .unwrap();
    let key = metadata.key();
    let def = SourceDefinition::new(wav.path().to_path_buf(), metadata, limits)
        .with_verified_manifest(reader.manifest().clone());
    let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(3), vec![def]).unwrap();
    runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
    let footprint = runtime.catalog_footprint().unwrap();
    assert_eq!(
        footprint.pitch_scratch_bytes,
        66_048 * std::mem::size_of::<[f32; 2]>()
    );
    assert!(footprint.kernel_table_bytes <= footprint.kernel_peak_bound_bytes);
    runtime.request_page(key, index);
    assert!(wait_for_pages(
        &mut runtime,
        &[PageKey {
            source: key,
            page_index: index
        }],
        Duration::from_secs(3)
    ));
    let page = runtime.page(key, index).unwrap();
    assert_eq!(page.frames.len() as u64, FRAMES - index * 4096);
    let pitch =
        hex_warp::PitchWindow::new(FRAMES, -700, hex_warp::Resampler::V2, &mut || false).unwrap();
    let range = pitch
        .required_source_range(index * 4096, page.frames.len())
        .unwrap();
    let mut source = vec![[0.0; 2]; (range.end - range.start) as usize];
    source[(impulse_frame - range.start) as usize] = [0.5, -0.25];
    let mut expected = vec![[0.0; 2]; page.frames.len()];
    pitch
        .render(
            index * 4096,
            range.start,
            &source,
            &mut expected,
            &mut || false,
        )
        .unwrap();
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(page.frames, expected);
    assert!(runtime.shutdown().unwrap().accounting_complete);
}

#[test]
fn pitched_worker_refuses_filter_budget_before_publication() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let limits = reader_limits(pcm.len());
    let reader = VerifiedWavReader::open(wav.path(), limits).unwrap();
    let metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(403, asset_id(pcm.len(), 48_000)),
        48_000,
        48_000,
        pcm.len() as u64,
        pcm.len() as u64,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap()
    .with_plain_pitch(700, hex_project::WarpResampler::V2)
    .unwrap();
    let definition = SourceDefinition::new(wav.path().to_path_buf(), metadata, limits);
    let mut config = StreamConfig::with_pool_pages(3);
    config.max_kernel_table_bytes =
        hex_warp::PitchWindow::filter_reservation_bytes(hex_warp::Resampler::V2) - 1;
    assert!(matches!(
        StreamRuntime::new(config, vec![definition]),
        Err(hex_stream::StreamRuntimeError::InvalidConfiguration)
    ));
}
