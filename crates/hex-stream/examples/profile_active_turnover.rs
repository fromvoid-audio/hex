//! Measure real-worker page turnover during a finite 100-track headless render.
//! `--sustained` renders 128 pages per source, excluding the first four pages
//! from the render distribution; page requests remain serial at each boundary.
//! `--sustained-long` uses the same span at a distant offset in long sparse files.
//! Headless timings do not establish physical audio-device deadlines.
use hex_engine::{
    Clip, MediaPages, Project as EngineProject, Renderer, StreamSourceKey, StreamedSourceMetadata,
    Track,
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntime,
};
use std::{
    error::Error,
    fs,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SOURCES: usize = 100;
const DEFAULT_PAGES: usize = 4;
const SUSTAINED_PAGES: usize = 128;
const SUSTAINED_WARMUP_PAGES: usize = 4;
const LONG_SOURCE_FRAMES: usize = 17_000_000;
const LONG_FIRST_PAGE: usize = 2048;
const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const PAGE: usize = hex_engine::STREAM_PAGE_FRAMES;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn percentile(sorted: &[u128], percent: usize) -> u128 {
    sorted[(sorted.len() * percent).div_ceil(100).max(1) - 1]
}
fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let (pages_count, long_source) = match arguments.next().as_deref() {
        None => (DEFAULT_PAGES, false),
        Some(argument) if argument == "--sustained" => (SUSTAINED_PAGES, false),
        Some(argument) if argument == "--sustained-long" => (SUSTAINED_PAGES, true),
        _ => return Err("usage: profile_active_turnover [--sustained|--sustained-long]".into()),
    };
    if arguments.next().is_some() {
        return Err("usage: profile_active_turnover [--sustained|--sustained-long]".into());
    }
    let warmup_pages = if pages_count == SUSTAINED_PAGES {
        SUSTAINED_WARMUP_PAGES
    } else {
        0
    };
    let setup = Instant::now();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-active-turnover-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    let frames = pages_count * PAGE;
    let source_frames = if long_source {
        LONG_SOURCE_FRAMES
    } else {
        frames
    };
    let first_page = if long_source { LONG_FIRST_PAGE } else { 0 };
    let source_offset = first_page * PAGE;
    let source_bytes = source_frames * 4;
    let mut wav = Vec::with_capacity(44 + frames * 4);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&u32::try_from(36 + source_bytes)?.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&u32::try_from(source_bytes)?.to_le_bytes());
    let mut oracle = Vec::with_capacity(frames);
    for i in 0..frames {
        let left = ((i % 101) as i16 - 50) * 400;
        let right = (50 - (i % 97) as i16) * 300;
        wav.extend_from_slice(&left.to_le_bytes());
        wav.extend_from_slice(&right.to_le_bytes());
        let sample = [left as f32 / 32768.0, right as f32 / 32768.0];
        let mut sum = [0.0_f32; 2];
        for _ in 0..SOURCES {
            sum[0] += sample[0];
            sum[1] += sample[1];
        }
        oracle.push(sum);
    }
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(
            (1024 * 1024).max(u64::try_from(44 + source_bytes)?),
            source_frames as u64,
            128,
            8192,
        ),
        1024 * 1024,
    );
    let mut editor = Editor::new(Project::new(RATE)?, SOURCES + 1)?;
    let mut paths = Vec::with_capacity(SOURCES);
    let mut assets = Vec::with_capacity(SOURCES);
    for index in 0..SOURCES {
        let path = fixture.0.join(format!("source-{index:03}.wav"));
        if long_source {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(&wav[..44])?;
            file.set_len(u64::try_from(44 + source_bytes)?)?;
            file.seek(SeekFrom::Start(u64::try_from(44 + source_offset * 4)?))?;
            file.write_all(&wav[44..])?;
        } else {
            fs::write(&path, &wav)?;
        }
        let Created::Imported { asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: format!("source {index:03}"),
            path: path.clone(),
            sample_rate: RATE,
            frames: source_frames as u64,
        })?
        else {
            return Err("unexpected import result".into());
        };
        paths.push(path);
        assets.push(asset);
    }
    let fingerprint = *VerifiedWavReader::open(&paths[0], limits)?.fingerprint();
    let mut definitions = Vec::with_capacity(SOURCES);
    let mut tracks = Vec::with_capacity(SOURCES);
    let mut keys = Vec::with_capacity(SOURCES);
    for (path, asset) in paths.iter().zip(assets) {
        let key = StreamSourceKey::new(91, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            RATE,
            RATE,
            source_frames as u64,
            source_frames as u64,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )?;
        definitions.push(SourceDefinition::new(
            path.clone(),
            metadata.clone(),
            limits,
        ));
        let clip = Clip::streamed_region(
            Arc::new(metadata),
            0,
            frames as u64,
            source_offset as u64,
            frames as u64,
            false,
        )?;
        tracks.push(Track::new(vec![clip], 1.0, false)?);
        keys.push(key);
    }
    let setup_time = setup.elapsed();
    let mut config = StreamConfig::with_pool_pages(SOURCES * 2 + 1);
    config.source_scratch_frames = 8192;
    let catalog_start = Instant::now();
    let mut runtime = StreamRuntime::new(config, definitions)?;
    runtime.wait_until_ready(Duration::from_secs(30))?;
    let catalog_time = catalog_start.elapsed();
    let page_keys = |page: usize| -> Vec<PageKey> {
        keys.iter()
            .map(|source| PageKey {
                source: *source,
                page_index: (first_page + page) as u64,
            })
            .collect()
    };
    let pages: Vec<_> = (0..pages_count).map(page_keys).collect();
    let mut request_time = Duration::ZERO;
    let mut wait_time = Duration::ZERO;
    let mut capacity_retries = 0_usize;
    let placeholder = StreamFailure {
        key: keys[0].into(),
        page_index: 0,
        kind: StreamFailureKind::CapacityExhausted,
    };
    let mut failures = [placeholder; SOURCES];
    let mut render_times = Vec::with_capacity(frames / BLOCK);
    let mut output = vec![[0.0; 2]; frames];
    let project = EngineProject::new(RATE, tracks)?;
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let turnover_start = Instant::now();
    for page in 0..pages_count {
        let deadline = Instant::now() + Duration::from_secs(30);
        for key in &pages[page] {
            let t = Instant::now();
            runtime.request_page(key.source, key.page_index);
            runtime.wake_worker();
            request_time += t.elapsed();
            let t = Instant::now();
            while !runtime.pin_pages(std::slice::from_ref(key)) {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "page {page} did not become ready; stats {:?}, retries {capacity_retries}",
                        runtime.stats()
                    )
                    .into());
                }
                runtime.pump_ready(128);
                let count = runtime.take_failures(&mut failures);
                for failure in &failures[..count] {
                    match (failure.kind, failure.key) {
                        (
                            StreamFailureKind::CapacityExhausted,
                            hex_engine::StreamResourceKey::Audio(source),
                        ) => {
                            runtime.request_page(source, failure.page_index);
                            capacity_retries += 1;
                        }
                        _ => return Err(format!("unexpected worker failure: {failure:?}").into()),
                    }
                }
                if count > 0 {
                    runtime.wake_worker();
                }
                thread::sleep(Duration::from_micros(100));
            }
            wait_time += t.elapsed();
        }
        for block in output[page * PAGE..(page + 1) * PAGE].chunks_mut(BLOCK) {
            let t = Instant::now();
            runtime.pump_ready(128);
            renderer.render_with_pages(block, &mut runtime)?;
            let elapsed = t.elapsed();
            if page >= warmup_pages {
                render_times.push(elapsed.as_nanos());
            }
        }
        runtime.unpin_all();
    }
    let turnover_time = turnover_start.elapsed();
    if output != oracle {
        return Err("full rendered PCM differed from independent oracle".into());
    }
    let stats = runtime.stats();
    // Every direct request (including retries) and accepted prefetch is either
    // queued or coalesced. A long source can also prefetch beyond the measured
    // clip's final page, unlike a source that ends at that boundary.
    let expected_attempts = u64::try_from(SOURCES * pages_count + capacity_retries)?
        .checked_add(stats.prefetched_requests)
        .ok_or("request attempt count overflow")?;
    let observed_attempts = stats
        .queued_requests
        .checked_add(stats.coalesced_requests)
        .ok_or("observed request count overflow")?;
    if stats.misses != 0
        || stats.rejected_requests != 0
        || stats.stale_pages != 0
        || observed_attempts != expected_attempts
    {
        return Err(format!("unexpected page demand statistics: {stats:?}").into());
    }
    render_times.sort_unstable();
    println!(
        "sources={SOURCES} source_frames={source_frames} first_page={first_page} pages_each={pages_count} frames={frames} rate={RATE} block={BLOCK} warmup_pages={warmup_pages} measured_blocks={} request_policy=serial_page_boundary pool_pages={} cache_capacity={} full_pcm=exact",
        render_times.len(),
        config.page_buffers,
        runtime.page_capacity()
    );
    println!(
        "fixture_ms={:.3} catalog_ms={:.3} turnover_ms={:.3} request_and_wake_ms={:.3} page_wait_ms={:.3} render_block_ns_p50={} p95={} p99={} max={} misses={} rejected={} stale={} queued={} prefetched={} coalesced={} capacity_retries={}",
        setup_time.as_secs_f64() * 1000.0,
        catalog_time.as_secs_f64() * 1000.0,
        turnover_time.as_secs_f64() * 1000.0,
        request_time.as_secs_f64() * 1000.0,
        wait_time.as_secs_f64() * 1000.0,
        percentile(&render_times, 50),
        percentile(&render_times, 95),
        percentile(&render_times, 99),
        render_times[render_times.len() - 1],
        stats.misses,
        stats.rejected_requests,
        stats.stale_pages,
        stats.queued_requests,
        stats.prefetched_requests,
        stats.coalesced_requests,
        capacity_retries
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
