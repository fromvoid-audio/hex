//! Matched release benchmark comparing single-threaded serial vs multi-core
//! `AudioWorkerPool` topological wave execution on a dense 64-track project.
//!
//! Run with:
//! `cargo run --release -p hex-audio --example profile_wave_parallelism`

use hex_audio::AudioWorkerPool;
use hex_engine::{
    Audio, Frame, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    WaveExecutor,
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 256;
const WARMUP_BLOCKS: usize = 32;
const MEASURED_BLOCKS: usize = 256;
const SOURCE_FRAMES: usize = BLOCK_FRAMES * (WARMUP_BLOCKS + MEASURED_BLOCKS + 4);

fn build_dense_project(track_count: usize, grouped: bool) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(SAMPLE_RATE).unwrap(), 128).unwrap();
    let mut media = BTreeMap::new();
    let mut tracks = Vec::with_capacity(track_count);

    for i in 0..track_count {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: format!("track-{i}"),
                path: format!("memory/track-{i}.wav").into(),
                source_sample_rate: SAMPLE_RATE,
                source_frames: SOURCE_FRAMES as u64,
                playback_frames: SOURCE_FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("expected imported track");
        };
        let frames: Vec<Frame> = (0..SOURCE_FRAMES)
            .map(|n| {
                let phase = n as f32 * (0.011 + i as f32 * 0.0009);
                [phase.sin() * 0.04, (phase * 1.07).cos() * 0.04]
            })
            .collect();
        media.insert(asset, Audio::new(SAMPLE_RATE, frames).unwrap());

        for state in [
            hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap(),
            hex_devices::filter::DEFINITION.default_state().unwrap(),
            hex_devices::compressor::COMPRESSOR_DEFINITION
                .default_state()
                .unwrap(),
        ] {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state,
                })
                .unwrap();
        }
        if !grouped {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: hex_devices::reverb::REVERB_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
        }
        tracks.push(track);
    }

    if grouped && track_count >= 16 {
        let group_count = 4;
        let per_group = track_count / group_count;
        let mut groups = Vec::with_capacity(group_count);
        for g in 0..group_count {
            let slice = &tracks[g * per_group..(g + 1) * per_group];
            let Created::Track(group) = editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(slice).unwrap(),
                    name: format!("group-{g}"),
                })
                .unwrap()
            else {
                panic!("expected group");
            };
            for state in [
                hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap(),
                hex_devices::compressor::COMPRESSOR_DEFINITION
                    .default_state()
                    .unwrap(),
            ] {
                editor
                    .execute(Command::InsertDevice {
                        track: group,
                        before: None,
                        state,
                    })
                    .unwrap();
            }
            groups.push(group);
        }
        for r in 0..4 {
            let Created::ReturnTrack(ret) = editor
                .execute(Command::AddReturnTrack {
                    name: format!("return-{r}"),
                })
                .unwrap()
            else {
                panic!("expected return");
            };
            editor
                .execute(Command::InsertReturnDevice {
                    return_track: ret,
                    before: None,
                    state: hex_devices::reverb::REVERB_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
            for &group in &groups {
                editor
                    .execute(Command::SetReturnSend {
                        track: group,
                        return_track: ret,
                        gain: 0.2,
                    })
                    .unwrap();
            }
        }
    }

    Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    )
}

fn measure_scenario(
    label: &str,
    revision: &Arc<PreparedRevision>,
    worker_count: usize,
) -> (f64, f64, Vec<Frame>) {
    let mut runtime = SessionPlayback::new(Arc::clone(revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut pool = AudioWorkerPool::new(worker_count, SAMPLE_RATE).unwrap();
    let mut block = [[0.0_f32; 2]; BLOCK_FRAMES];

    for _ in 0..WARMUP_BLOCKS {
        if worker_count == 0 {
            runtime.render(&mut block).unwrap();
        } else {
            runtime
                .render_with_wave_executor(&mut block, &mut pool)
                .unwrap();
        }
    }

    let mut durations_us = Vec::with_capacity(MEASURED_BLOCKS);
    let mut last_block = Vec::new();
    for b in 0..MEASURED_BLOCKS {
        let start = Instant::now();
        if worker_count == 0 {
            runtime.render(&mut block).unwrap();
        } else {
            runtime
                .render_with_wave_executor(&mut block, &mut pool)
                .unwrap();
        }
        durations_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
        if b + 1 == MEASURED_BLOCKS {
            last_block = block.to_vec();
        }
    }
    durations_us.sort_by(f64::total_cmp);
    let mean = durations_us.iter().sum::<f64>() / durations_us.len() as f64;
    let p95 = durations_us[(durations_us.len() * 95) / 100];
    let budget_us = (BLOCK_FRAMES as f64 / SAMPLE_RATE as f64) * 1_000_000.0;
    println!(
        "{label} (threads={}): mean={mean:.1} us, p95={p95:.1} us, budget={budget_us:.1} us ({:.1}% of callback budget)",
        pool.worker_count() + 1,
        (mean / budget_us) * 100.0
    );
    (mean, p95, last_block)
}

fn main() {
    let cores = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let workers = cores
        .saturating_sub(1)
        .min(hex_audio::worker_pool::MAX_WORKERS);
    println!(
        "hex wave parallelism benchmark: 64 tracks x native DSP chains, {} hz, {} frames/block, {} measured blocks (cores={cores}, workers={workers})",
        SAMPLE_RATE, BLOCK_FRAMES, MEASURED_BLOCKS
    );

    for (name, track_count, grouped) in [
        ("flat-64 (EQ8+Filter+Comp+Reverb)", 64, false),
        ("grouped-64->4-groups->4-returns (3 waves)", 64, true),
    ] {
        let revision = build_dense_project(track_count, grouped);
        let (serial_mean, _serial_p95, serial_last) =
            measure_scenario(&format!("{name} serial  "), &revision, 0);
        let (parallel_mean, _parallel_p95, parallel_last) =
            measure_scenario(&format!("{name} parallel"), &revision, workers);
        assert_eq!(
            serial_last
                .iter()
                .map(|f| [f[0].to_bits(), f[1].to_bits()])
                .collect::<Vec<_>>(),
            parallel_last
                .iter()
                .map(|f| [f[0].to_bits(), f[1].to_bits()])
                .collect::<Vec<_>>(),
            "{name}: parallel output must be bit-identical to serial"
        );
        println!(
            "  => {name} speedup: {:.2}x (bit-identical verified)\n",
            serial_mean / parallel_mean
        );
    }
}
