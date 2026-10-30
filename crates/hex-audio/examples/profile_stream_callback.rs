//! Headless prepared audio callback benchmark with verified streamed sources.
//! No physical device, native deadlines or listening acceptance is claimed.
use hex_audio::{prepared::PublicationPolicy, worker_fixture};
use hex_engine::{
    StreamSourceKey, StreamedSourceMetadata,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
    playback::{Playback, PlaybackCommand},
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{SourceDefinition, StreamConfig, StreamRuntime};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Acquire},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const SOURCES: usize = 100;
const PAGES: usize = 4;
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
    let args: Vec<_> = std::env::args().skip(1).collect();
    let pool_pages = match args.as_slice() {
        [] => SOURCES * 2 + 1,
        [size] => size.parse::<usize>()?,
        _ => return Err("usage: profile_stream_callback [pool pages]".into()),
    };
    if !(SOURCES + 1..=1024).contains(&pool_pages) {
        return Err("pool pages must be between 101 and 1024".into());
    }
    let setup = Instant::now();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-active-turnover-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    let frames = PAGES * PAGE;
    let mut wav = Vec::with_capacity(44 + frames * 4);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&u32::try_from(36 + frames * 4)?.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&u32::try_from(frames * 4)?.to_le_bytes());
    wav.resize(44 + frames * 4, 0);
    let mut oracle = vec![[0.0_f32; 2]; frames];
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(1024 * 1024, frames as u64, 128, 8192),
        1024 * 1024,
    );
    let mut editor = Editor::new(Project::new(RATE)?, SOURCES + 1)?;
    let mut paths = Vec::with_capacity(SOURCES);
    let mut assets = Vec::with_capacity(SOURCES);
    for index in 0..SOURCES {
        for (i, sum) in oracle.iter_mut().enumerate() {
            let left = (((i + index * 7) % 101) as i16 - 50) * 400;
            let right = (50 - ((i + index * 11) % 97) as i16) * 300;
            let offset = 44 + i * 4;
            wav[offset..offset + 2].copy_from_slice(&left.to_le_bytes());
            wav[offset + 2..offset + 4].copy_from_slice(&right.to_le_bytes());
            sum[0] += left as f32 / 32768.0;
            sum[1] += right as f32 / 32768.0;
        }
        let path = fixture.0.join(format!("source-{index:03}.wav"));
        fs::write(&path, &wav)?;
        let Created::Imported { asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: format!("source {index:03}"),
            path: path.clone(),
            sample_rate: RATE,
            frames: frames as u64,
        })?
        else {
            return Err("unexpected import result".into());
        };
        paths.push(path);
        assets.push(asset);
    }
    let mut definitions = Vec::with_capacity(SOURCES);
    let mut sources = BTreeMap::new();
    for (path, asset) in paths.iter().zip(assets) {
        let key = StreamSourceKey::new(91, asset);
        let fingerprint = *VerifiedWavReader::open(path, limits)?.fingerprint();
        let metadata = StreamedSourceMetadata::new(
            key,
            RATE,
            RATE,
            frames as u64,
            frames as u64,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )?;
        definitions.push(SourceDefinition::new(
            path.clone(),
            metadata.clone(),
            limits,
        ));
        sources.insert(
            asset,
            hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
        );
    }
    let setup_time = setup.elapsed();
    let mut config = StreamConfig::with_pool_pages(pool_pages);
    config.source_scratch_frames = 8192;
    let catalog_start = Instant::now();
    let mut runtime = StreamRuntime::new(config, definitions)?;
    runtime.wait_until_ready(Duration::from_secs(30))?;
    let catalog_time = catalog_start.elapsed();
    let domain = runtime.split().map_err(|_| "stream split failed")?;
    let cancelled = AtomicBool::new(false);
    let store = ResidentDeviceAssets::default();
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        editor.project(),
        100_000_000,
        &cancelled,
    )
    .map_err(|error| format!("resource preparation: {error:?}"))?;
    let revision =
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            editor.project(),
            &sources,
            91,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000_000,
            &cancelled,
        )?;
    let playback =
        Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources)?;
    let (mut output, mut backend) = worker_fixture::open(RATE)?;
    let control = output
        .publication
        .control()
        .ok_or("missing publication control")?;
    control.configure_device_resource_publication_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 4096,
            handle_rows: 4096,
            root_rows: 128,
            max_retained_bytes: 128 * 1024 * 1024,
            max_construction_peak_bytes: 128 * 1024 * 1024,
        },
    )?;
    control.configure_ordinary_streaming_on_control(
        hex_audio::AudioCandidateDomainReservation {
            max_retained_data_bytes: 128 * 1024 * 1024,
            max_construction_peak_data_bytes: 128 * 1024 * 1024,
        },
        SOURCES * 2,
        2 * 1024 * 1024,
    )?;
    let token = control
        .register_audio_candidate_domain_with_readiness(
            domain,
            hex_stream::StreamReadinessReservation {
                extents_per_role: SOURCES * 2,
            },
        )
        .map_err(|error| format!("domain registration: {}", error.reason))?;
    backend.render(&mut [[0.0; 2]; BLOCK])?;
    let revision_id = control
        .publish_with_admission_and_source_on_control(
            playback,
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 1,
            },
            |_| Ok::<_, ()>(()),
        )
        .map_err(|error| format!("publication: {error:?}"))?;
    let mut client = output.take_client().ok_or("missing client")?;
    client
        .command(PlaybackCommand::Play)
        .map_err(|_| "play queue full")?;
    let mut block = [[0.0; 2]; BLOCK];
    let mut elapsed = Vec::with_capacity(frames / BLOCK);
    let mut received = 0_usize;
    let mut silent_blocks = 0_usize;
    let mut silent_by_page = [0_usize; PAGES];
    let mut buffering_time = Duration::ZERO;
    let period = Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
    let run_start = Instant::now();
    let mut next_callback = run_start;
    let mut late_blocks = 0_usize;
    let deadline = Instant::now() + Duration::from_secs(30);
    while received < frames {
        if Instant::now() >= deadline {
            return Err(format!(
                "callback stalled at {received} frames after {silent_blocks} silent blocks"
            )
            .into());
        }
        let before = client
            .status()
            .ok_or("status closed")?
            .performance_frame
            .load(Acquire);
        let t = Instant::now();
        let mut result = Ok(());
        let count = allocation_counter::measure(|| {
            result = backend.render(&mut block);
        });
        let duration = t.elapsed();
        result?;
        if (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current,
        ) != (0, 0, 0, 0)
        {
            return Err(format!("callback allocated or freed: {count:?}").into());
        }
        let after = client
            .status()
            .ok_or("status closed")?
            .performance_frame
            .load(Acquire);
        let advanced = usize::try_from(after.checked_sub(before).ok_or("clock moved backwards")?)?;
        if advanced == 0 {
            if block != [[0.0; 2]; BLOCK] {
                return Err("buffering emitted non-silence".into());
            }
            silent_blocks += 1;
            silent_by_page[received / PAGE] += 1;
            buffering_time += duration;
            thread::yield_now();
        } else {
            if advanced != BLOCK || received + advanced > frames {
                return Err(format!("unexpected progress {advanced}").into());
            }
            if block.as_slice() != &oracle[received..received + advanced] {
                return Err(format!("PCM mismatch at {received}").into());
            }
            received += advanced;
            elapsed.push(duration.as_nanos());
        }
        let status = client.status().ok_or("status closed")?;
        if status.rejected_revision().is_some() || status.stream_readiness_refused.load(Acquire) {
            return Err("stream publication or readiness refused".into());
        }
        // Pacing is outside the measured callback and does not emulate a device.
        next_callback += period;
        if let Some(delay) = next_callback.checked_duration_since(Instant::now()) {
            thread::sleep(delay);
        } else {
            late_blocks += 1;
        }
    }
    if client
        .status()
        .ok_or("status closed")?
        .applied_revision
        .load(Acquire)
        != revision_id
    {
        return Err("revision not applied".into());
    }
    elapsed.sort_unstable();
    println!(
        "sources={SOURCES} pages_each={PAGES} frames={frames} rate={RATE} block={BLOCK} pool_pages={pool_pages} warmup=none pacing=simulated full_pcm=exact allocations=zero callback=prepared_headless"
    );
    println!(
        "fixture_ms={:.3} catalog_ms={:.3} run_wall_ms={:.3} buffering_callback_ms={:.3} silent_blocks={silent_blocks} silent_by_page={silent_by_page:?} late_blocks={late_blocks} callback_ns_p50={} p95={} p99={} max={}",
        setup_time.as_secs_f64() * 1000.0,
        catalog_time.as_secs_f64() * 1000.0,
        run_start.elapsed().as_secs_f64() * 1000.0,
        buffering_time.as_secs_f64() * 1000.0,
        percentile(&elapsed, 50),
        percentile(&elapsed, 95),
        percentile(&elapsed, 99),
        elapsed[elapsed.len() - 1]
    );
    let control = output
        .publication
        .control()
        .ok_or("missing publication control")?;
    if !control.retire_audio_candidate_domain(token) {
        return Err("domain retirement refused".into());
    }
    control.request_shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        backend.render(&mut block)?;
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(token) {
            if !report.worker_joined || !report.accounting_complete {
                return Err("incomplete stream custody".into());
            }
            println!(
                "shutdown_joined={} accounted_buffers={}/{}",
                report.worker_joined,
                report.queued_pages_reclaimed
                    + report.held_return_pages_reclaimed
                    + report.worker_pages_reclaimed,
                report.page_buffers
            );
            break;
        }
        if Instant::now() >= deadline {
            return Err("domain did not retire".into());
        }
        thread::yield_now();
    }
    client.close();
    backend.close();
    output.publication.close();
    Ok(())
}
