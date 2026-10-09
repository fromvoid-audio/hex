//! Measure prepared playback around enabled, disabled, and one-frame loop wraps.
//! This in-memory render profile is not an audio-device deadline test.
use hex_engine::{
    Audio, Clip, Frame, Project, Track,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{ClipFade, LoopRegion};
use std::{error::Error, hint::black_box, time::Instant};

const TRACKS: usize = 100;
const CLIPS_PER_TRACK: usize = 100;
const BLOCK_FRAMES: usize = 128;
const WARMUP_BLOCKS: usize = 40;
const MEASURED_BLOCKS: usize = 200;
const SAMPLE_RATE: u32 = 48_000;
const SOURCE_FRAMES: usize = 64;
const SPARSE_START: u64 = 20_000;
const ARRANGEMENT_FRAMES: u64 = 500_000;
const FADE_FRAMES: u64 = 4_800;

#[derive(Clone, Copy)]
struct Scenario {
    label: &'static str,
    loop_region: Option<LoopRegion>,
}

fn project(
    loop_region: Option<LoopRegion>,
    fades: bool,
    phase: bool,
) -> Result<Project, hex_engine::Invalid> {
    let mut tracks = Vec::with_capacity(TRACKS);
    for track_index in 0..TRACKS {
        let source = (0..SOURCE_FRAMES)
            .map(|frame| {
                let left = ((track_index * 17 + frame * 11) % 97) as f32 / 128.0 - 0.25;
                let right = ((track_index * 7 + frame * 19) % 89) as f32 / 128.0 - 0.2;
                [left, right]
            })
            .collect::<Vec<Frame>>();
        let audio = Audio::new(SAMPLE_RATE, source)?;
        let gain = 0.25 + (track_index % 5) as f32 * 0.125;
        let mut clips = Vec::with_capacity(CLIPS_PER_TRACK);
        for clip_index in 0..CLIPS_PER_TRACK {
            if clip_index == 0 {
                if fades {
                    let fade = ClipFade::new(ARRANGEMENT_FRAMES, 0, FADE_FRAMES, FADE_FRAMES)
                        .map_err(|error| hex_engine::Invalid(error.0))?;
                    clips.push(Clip::region_with_fade(
                        audio.clone(),
                        0,
                        ARRANGEMENT_FRAMES,
                        0,
                        SOURCE_FRAMES as u64,
                        true,
                        fade,
                    )?);
                } else {
                    clips.push(Clip::new(audio.clone(), 0, ARRANGEMENT_FRAMES, true)?);
                }
            } else {
                let start = SPARSE_START
                    + ((track_index * 1_619 + clip_index * 4_093 + track_index * clip_index * 31)
                        as u64
                        % (ARRANGEMENT_FRAMES - SPARSE_START));
                let duration = match clip_index % 3 {
                    0 => 16,
                    1 => 32,
                    _ => 64,
                };
                clips.push(Clip::new(
                    audio.clone(),
                    start,
                    duration,
                    (track_index + clip_index) % 3 == 0,
                )?);
            }
        }
        if phase {
            let active = clips.remove(0).with_source_phase(17)?;
            clips.insert(0, active);
        }
        tracks.push(Track::new(clips, gain, false)?);
    }
    Project::with_loop_region(SAMPLE_RATE, tracks, loop_region)
}

fn digest(frames: &[Frame]) -> u64 {
    frames.iter().fold(0xcbf2_9ce4_8422_2325, |hash, frame| {
        let hash = (hash ^ u64::from(frame[0].to_bits())).wrapping_mul(0x100_0000_01b3);
        (hash ^ u64::from(frame[1].to_bits())).wrapping_mul(0x100_0000_01b3)
    })
}

fn report(label: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2] as f64 / 1_000.0;
    let p95 = samples[samples.len() * 95 / 100] as f64 / 1_000.0;
    let max = samples[samples.len() - 1] as f64 / 1_000.0;
    println!("{label}: median={p50:.2}µs p95={p95:.2}µs max={max:.2}µs");
}

fn measure(scenario: Scenario, fades: bool, phase: bool) -> Result<(), Box<dyn Error>> {
    let mut playback = Playback::new(project(scenario.loop_region, fades, phase)?);
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; BLOCK_FRAMES];

    for _ in 0..WARMUP_BLOCKS {
        playback.apply(PlaybackCommand::Seek(0));
        playback.render(black_box(&mut output))?;
    }

    let mut times = Vec::with_capacity(MEASURED_BLOCKS);
    for _ in 0..MEASURED_BLOCKS {
        playback.apply(PlaybackCommand::Seek(0));
        let started = Instant::now();
        playback.render(black_box(&mut output))?;
        times.push(started.elapsed().as_nanos());
    }
    let output_digest = digest(black_box(&output));
    println!(
        "{}: {TRACKS} tracks × {CLIPS_PER_TRACK} clips ({} sustained active, {} sparse), {SAMPLE_RATE} hz, {BLOCK_FRAMES}-frame blocks, {WARMUP_BLOCKS} warmup + {MEASURED_BLOCKS} measured, device=none, hardware=model not recorded",
        scenario.label,
        TRACKS,
        TRACKS * (CLIPS_PER_TRACK - 1),
    );
    report("render time", &mut times);
    if fades {
        println!(
            "fade workload: {TRACKS} active clip envelopes, {FADE_FRAMES}-frame linear fade-in/out, source loop phase independent"
        );
    }
    if phase {
        println!(
            "phase workload: {TRACKS} active source loops, 17-frame phase in 64-frame interval"
        );
    }
    println!("output digest: {output_digest:016x}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!(
        "compiled clip stride: {} bytes",
        std::mem::size_of::<Clip>()
    );
    let mut fades = false;
    let mut phase = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fades" if !fades => fades = true,
            "--phase" if !phase => phase = true,
            _ => return Err("usage: profile_loop [--fades] [--phase]".into()),
        }
    }
    let scenarios = [
        Scenario {
            label: "loop disabled",
            loop_region: Some(LoopRegion::new(0, 1_024, false)?),
        },
        Scenario {
            label: "1,024-frame loop",
            loop_region: Some(LoopRegion::new(0, 1_024, true)?),
        },
        Scenario {
            label: "1-frame loop",
            loop_region: Some(LoopRegion::new(0, 1, true)?),
        },
    ];
    for scenario in scenarios {
        measure(scenario, fades, phase)?;
    }
    Ok(())
}
