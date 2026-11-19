//! Profile 100 independent sparse 17-million-frame sources and active tracks.
//! Each source has its own path and canonical asset ID; equal fixture bytes
//! permit one independently computed source fingerprint to be reused.
use hex_engine::{
    Clip, MediaPages, Project as EngineProject, Renderer, StreamSourceKey, StreamedSourceMetadata,
    Track,
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{EndpointStats, PageKey, SourceDefinition, StreamConfig, StreamRuntime};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SOURCE_COUNT: usize = 100;
const SOURCE_FRAMES: u64 = 17_000_000;
const RATE: u32 = 48_000;
const ACTIVE_PAGE: u64 = 4095;
const PAGE_FRAMES: usize = hex_engine::STREAM_PAGE_FRAMES;
const MEASURED_RUNS: usize = 100;
const BLOCK_FRAMES: usize = 128;

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn create() -> Result<Self, Box<dyn Error>> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path =
            std::env::temp_dir().join(format!("hex-stream-long-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn active_pcm() -> Vec<[f32; 2]> {
    (0..PAGE_FRAMES)
        .map(|frame| {
            let left = (frame as i32 % 101 - 50) * 400;
            let right = (50 - frame as i32 % 97) * 300;
            [left as f32 / 32768.0, right as f32 / 32768.0]
        })
        .collect()
}

fn write_sparse_source(path: &std::path::Path, active: &[[f32; 2]]) -> Result<(), Box<dyn Error>> {
    let data_bytes = SOURCE_FRAMES
        .checked_mul(4)
        .ok_or("WAV data size overflow")?;
    let riff_size = 36_u64.checked_add(data_bytes).ok_or("RIFF size overflow")?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&u32::try_from(riff_size)?.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * 4).to_le_bytes())?;
    file.write_all(&4_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&u32::try_from(data_bytes)?.to_le_bytes())?;
    file.set_len(
        44_u64
            .checked_add(data_bytes)
            .ok_or("WAV file size overflow")?,
    )?;
    let byte_offset = 44_u64
        .checked_add(
            ACTIVE_PAGE
                .checked_mul(PAGE_FRAMES as u64)
                .ok_or("active offset overflow")?
                .checked_mul(4)
                .ok_or("active byte offset overflow")?,
        )
        .ok_or("active offset overflow")?;
    file.seek(SeekFrom::Start(byte_offset))?;
    for frame in active {
        let left = (frame[0] * 32768.0) as i16;
        let right = (frame[1] * 32768.0) as i16;
        file.write_all(&left.to_le_bytes())?;
        file.write_all(&right.to_le_bytes())?;
    }
    file.sync_all()?;
    Ok(())
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn validate_stream_stats(stats: EndpointStats) -> Result<(), Box<dyn Error>> {
    // Direct requests and speculative lookahead attempts must each be queued
    // or coalesced; a rejected request is never an acceptable explanation.
    let expected = u64::try_from(SOURCE_COUNT)?
        .checked_add(stats.prefetched_requests)
        .ok_or("request attempt count overflow")?;
    let observed = stats
        .queued_requests
        .checked_add(stats.coalesced_requests)
        .ok_or("observed request count overflow")?;
    if stats.misses != 0
        || stats.rejected_requests != 0
        || stats.stale_pages != 0
        || observed != expected
    {
        return Err(format!("unexpected warmed 100-track stream statistics: {stats:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmed_stats_count_lookahead_without_hiding_failures() {
        let direct = EndpointStats {
            queued_requests: 100,
            ..EndpointStats::default()
        };
        assert!(validate_stream_stats(direct).is_ok());
        let prefetch = EndpointStats {
            queued_requests: 101,
            prefetched_requests: 1,
            ..direct
        };
        assert!(validate_stream_stats(prefetch).is_ok());
        assert!(
            validate_stream_stats(EndpointStats {
                coalesced_requests: 1,
                queued_requests: 100,
                ..prefetch
            })
            .is_ok()
        );
        for invalid in [
            EndpointStats {
                misses: 1,
                ..prefetch
            },
            EndpointStats {
                rejected_requests: 1,
                ..prefetch
            },
            EndpointStats {
                stale_pages: 1,
                ..prefetch
            },
            EndpointStats {
                queued_requests: 102,
                ..prefetch
            },
            EndpointStats {
                queued_requests: 100,
                ..prefetch
            },
            EndpointStats {
                prefetched_requests: u64::MAX,
                ..direct
            },
        ] {
            assert!(validate_stream_stats(invalid).is_err(), "{invalid:?}");
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixture = FixtureDirectory::create()?;
    let mut editor = Editor::new(Project::new(RATE)?, SOURCE_COUNT + 1)?;
    let active = active_pcm();
    let mut paths = Vec::new();
    let mut assets = Vec::new();
    paths.try_reserve_exact(SOURCE_COUNT)?;
    assets.try_reserve_exact(SOURCE_COUNT)?;
    for index in 0..SOURCE_COUNT {
        let path = fixture.0.join(format!("source-{index:03}.wav"));
        write_sparse_source(&path, &active)?;
        let result = editor.execute(Command::ImportAudioTrack {
            name: format!("source {index:03}"),
            path: path.clone(),
            sample_rate: RATE,
            frames: SOURCE_FRAMES,
        })?;
        let asset = match result {
            Created::Imported { asset, .. } => asset,
            other => return Err(format!("unexpected import result: {other:?}").into()),
        };
        paths.push(path);
        assets.push(asset);
    }

    let reader_limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(80 * 1024 * 1024, SOURCE_FRAMES, 128, 8192),
        2 * 1024 * 1024,
    );
    let started = Instant::now();
    let first_reader = VerifiedWavReader::open(&paths[0], reader_limits)?;
    let fingerprint_scan = started.elapsed();
    let fingerprint = *first_reader.fingerprint();
    drop(first_reader);

    let mut definitions = Vec::new();
    let mut metadata = Vec::new();
    definitions.try_reserve_exact(SOURCE_COUNT)?;
    metadata.try_reserve_exact(SOURCE_COUNT)?;
    for index in 0..SOURCE_COUNT {
        let key = StreamSourceKey::new(91, assets[index]);
        let source = Arc::new(StreamedSourceMetadata::new(
            key,
            RATE,
            RATE,
            SOURCE_FRAMES,
            SOURCE_FRAMES,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )?);
        metadata.push(Arc::clone(&source));
        definitions.push(SourceDefinition::new(
            paths[index].clone(),
            (*source).clone(),
            reader_limits,
        ));
    }

    let mut config = StreamConfig::with_pool_pages(1024);
    config.max_sources = SOURCE_COUNT;
    config.max_manifest_bytes = 8 * 1024 * 1024;
    config.max_reader_scratch_bytes = 8 * 1024 * 1024;
    config.source_scratch_frames = 8192;
    let started = Instant::now();
    let mut runtime = StreamRuntime::new(config, definitions)?;
    runtime.wait_until_ready(Duration::from_secs(300))?;
    let worker_catalog = started.elapsed();
    let footprint = runtime
        .catalog_footprint()
        .ok_or("missing catalog footprint")?;

    let page_keys: Vec<_> = metadata
        .iter()
        .map(|source| PageKey {
            source: source.key(),
            page_index: ACTIVE_PAGE,
        })
        .collect();
    let started = Instant::now();
    for key in &page_keys {
        runtime.request_page(key.source, key.page_index);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        runtime.pump_ready(128);
        if runtime.pin_pages(&page_keys) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    if !runtime.pin_pages(&page_keys) {
        return Err("all 100 far active pages did not become resident".into());
    }
    let far_pages_prepare = started.elapsed();

    let active_start = ACTIVE_PAGE * PAGE_FRAMES as u64;
    let mut tracks = Vec::new();
    tracks.try_reserve_exact(SOURCE_COUNT)?;
    for source in &metadata {
        let clip = Clip::streamed_region(
            Arc::clone(source),
            0,
            PAGE_FRAMES as u64,
            active_start,
            PAGE_FRAMES as u64,
            false,
        )?;
        tracks.push(Track::new(vec![clip], 1.0, false)?);
    }
    let project = EngineProject::new(RATE, tracks)?;
    let mut renderer = Renderer::new(&project);
    renderer.play();

    // Independent scalar oracle: sum each track's source window in canonical
    // track order with the track's unity coefficients.
    let mut oracle = vec![[0.0; 2]; PAGE_FRAMES];
    for _track in 0..SOURCE_COUNT {
        for (mixed, source) in oracle.iter_mut().zip(&active) {
            mixed[0] += source[0];
            mixed[1] += source[1];
        }
    }
    let mut render_window = vec![[0.0; 2]; PAGE_FRAMES];
    let mut measured = Vec::new();
    measured.try_reserve_exact(MEASURED_RUNS)?;
    for _ in 0..MEASURED_RUNS {
        renderer.seek(0); // explicit retrigger remains outside the timed region
        let started = Instant::now();
        for block in render_window.chunks_mut(BLOCK_FRAMES) {
            renderer.render_with_pages(block, &mut runtime)?;
        }
        measured.push(started.elapsed().as_nanos());
        if render_window != oracle {
            return Err("100-track output differed from independent scalar oracle".into());
        }
    }
    measured.sort_unstable();
    let stats = runtime.stats();
    validate_stream_stats(stats)?;

    let logical_input_bytes = SOURCE_COUNT as u64 * (44 + SOURCE_FRAMES * 4);
    println!(
        "workload=100_independent_sparse_sources_and_tracks frames_each={SOURCE_FRAMES} seconds_each={:.2} active_source_frame={active_start} rate={RATE} blocks_per_run={} runs={MEASURED_RUNS}",
        SOURCE_FRAMES as f64 / f64::from(RATE),
        PAGE_FRAMES / BLOCK_FRAMES
    );
    println!(
        "logical_input_bytes={} first_source_fingerprint_ms={:.3} worker_catalog_hash_all_sources_ms={:.3} all_far_pages_prepare_ms={:.3}",
        logical_input_bytes,
        fingerprint_scan.as_secs_f64() * 1000.0,
        worker_catalog.as_secs_f64() * 1000.0,
        far_pages_prepare.as_secs_f64() * 1000.0
    );
    println!(
        "render_4096_frames_100_tracks_ns_p50={} p95={} p99={} max={}",
        percentile(&measured, 50),
        percentile(&measured, 95),
        percentile(&measured, 99),
        measured[measured.len() - 1]
    );
    println!(
        "manifest_bytes={} unique_kernel_payload_bytes={} kernel_peak_bound_bytes={} verified_reader_scratch_bytes={} worker_source_scratch_bytes={} page_pool_bytes={} active_misses={} queued_requests={} prefetched_requests={} coalesced_requests={} rejected_requests={} stale_pages={}",
        footprint.manifest_bytes,
        footprint.kernel_table_bytes,
        footprint.kernel_peak_bound_bytes,
        footprint.verified_reader_scratch_bytes,
        footprint.source_scratch_bytes,
        config.page_buffers * PAGE_FRAMES * std::mem::size_of::<hex_engine::Frame>(),
        stats.misses,
        stats.queued_requests,
        stats.prefetched_requests,
        stats.coalesced_requests,
        stats.rejected_requests,
        stats.stale_pages
    );
    let report = runtime.shutdown()?;
    if !report.worker_joined || !report.accounting_complete {
        return Err("worker or page custody did not finish".into());
    }
    println!(
        "shutdown_joined={} accounted_buffers={}/{}",
        report.worker_joined,
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed,
        report.page_buffers
    );
    Ok(())
}
