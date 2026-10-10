//! Canonical release workload for the bounded Return routing path.
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, Renderer, compile};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, hint::black_box, mem::size_of, time::Instant};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
const TRACKS: usize = 100;
const RETURNS: usize = 16;
const WARMUP_BLOCKS: usize = 100;
const MEASURED_BLOCKS: usize = 300;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let samples: Vec<[f32; 2]> = (0..BLOCK)
        .map(|frame| {
            let phase = frame as f32 * 0.071;
            [phase.sin() * 0.2, (phase * 0.61).cos() * 0.15]
        })
        .collect();
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/profile-return.wav".into(),
        sample_rate: RATE,
        frames: BLOCK as u64,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected benchmark audio asset".into()),
    };

    let filter_state = UTILITY_DEFINITION.patch_parameter(
        &UTILITY_DEFINITION.default_state()?,
        parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    )?;
    let mut return_ids = Vec::with_capacity(RETURNS);
    for index in 0..RETURNS {
        let return_track = match editor.execute(Command::AddReturnTrack {
            name: format!("filter {index}"),
        })? {
            Created::ReturnTrack(return_track) => return_track,
            _ => return Err("expected benchmark Return".into()),
        };
        editor.execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: filter_state.clone(),
        })?;
        return_ids.push(return_track);
    }

    let mut track_count = 0;
    for index in 0..TRACKS {
        let track = match editor.execute(Command::AddTrack {
            name: format!("source {index}"),
        })? {
            Created::Track(track) => track,
            _ => return Err("expected benchmark source track".into()),
        };
        editor.execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 10_000_000,
            source_offset: 0,
            source_length: BLOCK as u64,
            looping: true,
        })?;
        for return_track in &return_ids {
            editor.execute(Command::SetReturnSend {
                track,
                return_track: *return_track,
                gain: 0.05,
            })?;
        }
        track_count += 1;
    }

    let media = BTreeMap::from([(asset, Audio::new(RATE, samples)?)]);
    let compiled = compile::project(editor.project(), &media)?;
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..WARMUP_BLOCKS {
        renderer.render(&mut output)?;
        black_box(&output);
    }

    let mut timings = Vec::with_capacity(MEASURED_BLOCKS);
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for _ in 0..MEASURED_BLOCKS {
        let start = Instant::now();
        renderer.render(&mut output)?;
        timings.push(start.elapsed().as_nanos());
        for sample in output.iter().flatten() {
            digest ^= u64::from(sample.to_bits());
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
        black_box(&output);
    }
    timings.sort_unstable();
    let percentile = |percent: usize| timings[(timings.len() - 1) * percent / 100] as f64 / 1_000.0;
    let scratch_bytes = RETURNS * BLOCK * size_of::<[f32; 2]>();
    let utility_runtime_bytes = size_of::<hex_devices::utility_filters::UtilityFilterProcessor>();
    let twin_history_bytes = RETURNS * 2 * utility_runtime_bytes;
    let send_pairs = track_count * RETURNS;
    let deadline_us = BLOCK as f64 / f64::from(RATE) * 1_000_000.0;

    println!(
        "return routing profile: {}-{} rate={RATE} block={BLOCK} warmup_blocks={WARMUP_BLOCKS} measured_blocks={MEASURED_BLOCKS}",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    println!(
        "workload: tracks={track_count} returns={RETURNS} sends={send_pairs} utility_dc_filters={RETURNS}"
    );
    println!(
        "render: p50={:.1}us p99={:.1}us max={:.1}us block_period={deadline_us:.1}us pcm_digest={digest:016x}",
        percentile(50),
        percentile(99),
        timings[timings.len() - 1] as f64 / 1_000.0,
    );
    println!(
        "bounded return storage: scratch={RETURNS}*{BLOCK}*{}={scratch_bytes} bytes; utility history twins={RETURNS}*2*{utility_runtime_bytes}={twin_history_bytes} declared bytes",
        size_of::<[f32; 2]>(),
    );
    Ok(())
}
