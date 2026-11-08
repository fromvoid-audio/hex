//! Canonical linear-chain preparation and resident rendering, without device I/O.
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, Frame, Renderer, compile};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{
    AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, IDENTITY_CONVERSION, Project,
    TrackContentSnapshot, TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const TRACKS: usize = 100;
const BLOCK: usize = 128;
const GAIN: f32 = 1.001;
const PREP_WARMUP: usize = 5;
const PREP_RUNS: usize = 20;
const RENDER_WARMUP: usize = 40;
const RENDER_RUNS: usize = 1_000;

fn fixture(count: usize) -> Result<Project, Box<dyn Error>> {
    let state = UTILITY_DEFINITION.patch_parameter(
        &UTILITY_DEFINITION.default_state()?,
        parameter::GAIN,
        ParameterValue::Float(GAIN),
    )?;
    let mut snapshot = Project::new(RATE)?.snapshot();
    snapshot.assets = vec![AssetSnapshot {
        original_source: None,
        id: 1,
        path: "memory/utility-profile.wav".into(),
        source_sample_rate: RATE,
        source_frames: 64,
        playback_frames: 64,
        conversion_version: IDENTITY_CONVERSION,
    }];
    snapshot.tracks = (0..TRACKS)
        .map(|track| -> Result<TrackSnapshot, hex_project::Invalid> {
            Ok(TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: 100 + track as u64,
                name: format!("track {track}"),
                gain: 0.5,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: (0..count)
                    .map(|device| DeviceSnapshot {
                        parent: None,
                        automation: Vec::new(),
                        comparison: None,
                        id: 300 + (track * count + device) as u64,
                        bypassed: false,
                        state: state.clone(),
                    })
                    .collect(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    active: true,
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: 200 + track as u64,
                    asset: 1,
                    start: 0,
                    duration: 1_000_000,
                    source_offset: 0,
                    source_length: 64,
                    source_phase: 0,
                    looping: true,
                    fade: ClipFade::neutral(1_000_000)?,
                }]),
                sends: Vec::new(),
            })
        })
        .collect::<Result<_, _>>()?;
    snapshot.next_id = 300 + (TRACKS * count) as u64;
    Ok(Project::from_snapshot(snapshot)?)
}

fn report(label: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let percentile = |p| samples[(samples.len() - 1) * p / 100] as f64 / 1_000.0;
    println!(
        "{label}: p50={:.2}µs p95={:.2}µs p99={:.2}µs max={:.2}µs",
        percentile(50),
        percentile(95),
        percentile(99),
        samples[samples.len() - 1] as f64 / 1_000.0
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    println!(
        "utility: {TRACKS} tracks, {RATE} hz, {BLOCK}-frame blocks, {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!(
        "preparation: {PREP_WARMUP} warm-up/{PREP_RUNS} measured; render: {RENDER_WARMUP} warm-up/{RENDER_RUNS} measured; no output device"
    );
    for count in [0, 1, 16, 64] {
        let project = fixture(count)?;
        let asset = project.assets().next().ok_or("profile asset missing")?.id();
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.01, -0.02]; 64])?)]);
        for _ in 0..PREP_WARMUP {
            black_box(compile::project(&project, &media)?);
        }
        let mut preparation = Vec::with_capacity(PREP_RUNS);
        for _ in 0..PREP_RUNS {
            let start = Instant::now();
            let prepared = compile::project(black_box(&project), black_box(&media))?;
            preparation.push(start.elapsed().as_nanos());
            black_box(&prepared);
            drop(prepared);
        }
        let prepared = compile::project(&project, &media)?;
        let mut renderer = Renderer::new(&prepared);
        renderer.play();
        let mut output = [[0.0; 2]; BLOCK];
        renderer.render(&mut output)?;
        let mut reference: Frame = [0.01, -0.02];
        for _ in 0..count {
            reference[0] *= GAIN;
            reference[1] *= GAIN;
        }
        let mut expected: Frame = [0.0; 2];
        for _ in 0..TRACKS {
            expected[0] += reference[0] * 0.5;
            expected[1] += reference[1] * 0.5;
        }
        for frame in &output {
            for channel in 0..2 {
                if !frame[channel].is_finite()
                    || (frame[channel] - expected[channel]).abs() > 0.000_1
                {
                    return Err(
                        "fused chain differs from independent sequential gain oracle".into(),
                    );
                }
            }
        }
        for _ in 0..RENDER_WARMUP {
            renderer.render(black_box(&mut output))?;
        }
        let mut rendering = Vec::with_capacity(RENDER_RUNS);
        for _ in 0..RENDER_RUNS {
            let start = Instant::now();
            renderer.render(black_box(&mut output))?;
            rendering.push(start.elapsed().as_nanos());
            black_box(&output);
        }
        println!("{count} instances per track; sequential oracle tolerance 0.0001");
        report("preparation", &mut preparation);
        report("render", &mut rendering);
    }
    Ok(())
}
