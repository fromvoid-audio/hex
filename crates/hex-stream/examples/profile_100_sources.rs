use hex_engine::{
    Audio, Clip, MediaPages, Project as EngineProject, Renderer, StreamSourceKey,
    StreamedSourceMetadata, Track,
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{SourceDefinition, StreamConfig, StreamRuntime};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SOURCE_COUNT: usize = 100;
const SOURCE_FRAMES: usize = 262_144;
const RATE: u32 = 48_000;

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn create() -> Result<Self, Box<dyn Error>> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path =
            std::env::temp_dir().join(format!("hex-stream-profile-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_source(
    path: &std::path::Path,
    source_index: usize,
) -> Result<Vec<[f32; 2]>, Box<dyn Error>> {
    let data_bytes = SOURCE_FRAMES
        .checked_mul(4)
        .ok_or("fixture byte size overflow")?;
    let riff_size = 36_usize
        .checked_add(data_bytes)
        .ok_or("fixture riff size overflow")?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut file = BufWriter::new(file);
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

    let mut source = Vec::new();
    source.try_reserve_exact(SOURCE_FRAMES)?;
    for index in 0..SOURCE_FRAMES {
        let left = ((index as i32 + source_index as i32 * 7) % 127 - 63) * 400;
        let right = (63 - (index as i32 + source_index as i32 * 11) % 127) * 300;
        file.write_all(&(left as i16).to_le_bytes())?;
        file.write_all(&(right as i16).to_le_bytes())?;
        source.push([left as f32 / 32768.0, right as f32 / 32768.0]);
    }
    file.flush()?;
    file.get_ref().sync_all()?;
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_fixtures_match_original_files_and_pcm() {
        // Full-file SHA-256 values and selected PCM samples were measured from
        // the original unbuffered writer, before changing its construction.
        let cases = [
            (
                0,
                "26814d83d4ecc50416fed5fc04de0ffe2bdec1e98d17336211a556467b2b2655",
                [[-25200, 18900], [-22000, 16500], [-19200, 14400]],
            ),
            (
                25,
                "d5659d1d93263a9030ad3bee37c4ff2922f2289c7e5177564f2fee3b3b084408",
                [[-6000, 12600], [-2800, 10200], [0, 8100]],
            ),
            (
                51,
                "74e67197f0891f12f0948208986c8dd128f9191d74e7b7e96c51cee32e2a9c42",
                [[16000, 3000], [19200, 600], [22000, -1500]],
            ),
        ];
        let fixture = FixtureDirectory::create().unwrap();
        for (index, expected_hash, samples) in cases {
            let path = fixture.path().join(format!("source-{index:03}.wav"));
            let oracle = write_source(&path, index).unwrap();
            let limits = VerifiedWavReaderLimits::new(
                WavReaderLimits::new(2 * 1024 * 1024, SOURCE_FRAMES as u64, 128, 65_536),
                1024 * 1024,
            );
            let reader = VerifiedWavReader::open(&path, limits).unwrap();
            let hash: String = reader
                .fingerprint()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(hash, expected_hash);
            assert_eq!(oracle.len(), SOURCE_FRAMES);
            for (frame, expected) in [0, 131_072, 262_143].into_iter().zip(samples) {
                assert_eq!(
                    oracle[frame],
                    expected.map(|sample| sample as f32 / 32768.0)
                );
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixture = FixtureDirectory::create()?;
    let mut editor = Editor::new(Project::new(RATE)?, SOURCE_COUNT + 1)?;
    let mut paths = Vec::new();
    paths.try_reserve_exact(SOURCE_COUNT)?;
    let mut asset_ids = Vec::new();
    asset_ids.try_reserve_exact(SOURCE_COUNT)?;
    let mut oracle_source = Vec::new();

    // Fixture creation is outside reported control/runtime preparation timings.
    for index in 0..SOURCE_COUNT {
        let path = fixture.path().join(format!("source-{index:03}.wav"));
        let source = write_source(&path, index)?;
        if index == 0 {
            oracle_source = source;
        }
        paths.push(path.clone());
        let result = editor.execute(Command::ImportAudioTrack {
            name: format!("source {index:03}"),
            path: path.clone(),
            sample_rate: RATE,
            frames: SOURCE_FRAMES as u64,
        })?;
        let asset = match result {
            Created::Imported { asset, .. } => asset,
            other => return Err(format!("unexpected fixture import result: {other:?}").into()),
        };
        asset_ids.push(asset);
    }

    let started = Instant::now();
    let mut definitions = Vec::new();
    definitions.try_reserve_exact(SOURCE_COUNT)?;
    let mut active_metadata = None;
    let mut active_asset = None;
    for (index, path) in paths.iter().enumerate() {
        let reader_limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(2 * 1024 * 1024, SOURCE_FRAMES as u64, 128, 65_536),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(path, reader_limits)?;
        let asset = asset_ids[index];
        let key = StreamSourceKey::new(1, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            RATE,
            RATE,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )?;
        if index == 0 {
            active_metadata = Some(metadata.clone());
            active_asset = Some(asset);
        }
        definitions.push(SourceDefinition::new(path.clone(), metadata, reader_limits));
    }
    let metadata_prepare = started.elapsed();

    let mut config = StreamConfig::with_pool_pages(8);
    config.max_sources = SOURCE_COUNT;
    config.max_reader_scratch_bytes = 8 * 1024 * 1024;
    let started = Instant::now();
    let mut runtime = StreamRuntime::new(config, definitions)?;
    runtime.wait_until_ready(Duration::from_secs(30))?;
    let worker_catalog = started.elapsed();
    let footprint = runtime
        .catalog_footprint()
        .ok_or("missing catalog footprint")?;

    let metadata = std::sync::Arc::new(active_metadata.ok_or("missing active source")?);
    let clip = Clip::streamed_region(
        metadata,
        0,
        SOURCE_FRAMES as u64,
        0,
        SOURCE_FRAMES as u64,
        false,
    )?;
    let project = EngineProject::new(RATE, vec![Track::new(vec![clip], 1.0, false)?])?;
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let active_key = StreamSourceKey::new(1, active_asset.ok_or("missing active asset")?);
    let started = Instant::now();
    runtime.request_page(active_key, 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        runtime.pump_ready(8);
        if runtime.pin_pages(&[hex_stream::PageKey {
            source: active_key,
            page_index: 0,
        }]) {
            runtime.unpin_all();
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    if !runtime.pin_pages(&[hex_stream::PageKey {
        source: active_key,
        page_index: 0,
    }]) {
        return Err("first active page did not become resident".into());
    }
    runtime.unpin_all();
    let first_page_ready = started.elapsed();

    let mut block = [[0.0; 2]; 128];
    let started = Instant::now();
    renderer.render_with_pages(&mut block, &mut runtime)?;
    let active_render = started.elapsed();
    let expected = Audio::new(RATE, oracle_source)?.frames()[..128].to_vec();
    if block.as_slice() != expected.as_slice() {
        return Err("active stream page differs from fixture PCM".into());
    }
    let stats = runtime.stats();
    if stats.misses != 0 || stats.queued_requests != 1 || stats.rejected_requests != 0 {
        return Err("warmed active render unexpectedly missed or rejected a page".into());
    }

    println!(
        "workload=100_distinct_sources source_frames_each={SOURCE_FRAMES} source_seconds_each={:.3} rate={RATE}",
        SOURCE_FRAMES as f64 / f64::from(RATE)
    );
    println!(
        "metadata_fingerprint_prepare_ms={:.3} worker_catalog_verify_ms={:.3} first_active_page_ms={:.3} active_render_128_frames_us={:.3}",
        metadata_prepare.as_secs_f64() * 1000.0,
        worker_catalog.as_secs_f64() * 1000.0,
        first_page_ready.as_secs_f64() * 1000.0,
        active_render.as_secs_f64() * 1_000_000.0
    );
    let page_pool_bytes = config.page_buffers
        * hex_engine::STREAM_PAGE_FRAMES
        * std::mem::size_of::<hex_engine::Frame>();
    println!(
        "unique_wav_payload_bytes={} manifest_bytes={} kernel_payload_bytes={} kernel_peak_bound_bytes={} verified_reader_scratch_bytes={} source_scratch_bytes={} page_pool_bytes={} active_misses={} queued_requests={}",
        SOURCE_COUNT * SOURCE_FRAMES * 4,
        footprint.manifest_bytes,
        footprint.kernel_table_bytes,
        footprint.kernel_peak_bound_bytes,
        footprint.verified_reader_scratch_bytes,
        footprint.source_scratch_bytes,
        page_pool_bytes,
        stats.misses,
        stats.queued_requests
    );

    let report = runtime.shutdown()?;
    if !report.accounting_complete {
        return Err("stream page ownership did not balance".into());
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
