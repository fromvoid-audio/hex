//! Per-block cost of finding Arrangement audio clips on dense tracks.
//! Workloads: 100 tracks × 1,000 short clips, and one track × 16,384 clips.
//! Clips are stored out of start order (as canonical edits append them), and
//! every tenth neighbour pair overlaps like a crossfade. The run renders the
//! whole arrangement sequentially in 256-frame blocks and prints per-block
//! timings plus an output digest, so before/after builds can be compared.
//! This is an in-memory renderer benchmark, not an audio-device deadline test.
use hex_engine::{Audio, Clip, Frame, Project, Renderer, Track};
use std::{hint::black_box, time::Instant};

const BLOCK_FRAMES: usize = 256;
const CLIP_FRAMES: u64 = 64;
const STRIDE_FRAMES: u64 = 80;
const PASSES: usize = 3;

fn build(tracks: usize, clips_per_track: usize) -> Result<(Project, u64), hex_engine::Invalid> {
    let mut engine_tracks = Vec::with_capacity(tracks);
    for track_index in 0..tracks {
        let source = (0..CLIP_FRAMES as usize)
            .map(|frame| {
                let left = ((track_index * 17 + frame * 11) % 97) as f32 / 128.0 - 0.25;
                let right = ((track_index * 7 + frame * 19) % 89) as f32 / 128.0 - 0.2;
                [left, right]
            })
            .collect::<Vec<_>>();
        let audio = Audio::new(48_000, source)?;
        let mut clips = Vec::with_capacity(clips_per_track);
        // A fixed odd-stride permutation keeps storage out of start order.
        for slot in 0..clips_per_track {
            let ordinal = (slot * 7_919 + track_index) % clips_per_track;
            let mut start = ordinal as u64 * STRIDE_FRAMES;
            if ordinal % 10 == 9 {
                // Overlap the previous clip's last 24 frames.
                start -= 24;
            }
            clips.push(Clip::new(audio.clone(), start, CLIP_FRAMES, false)?);
        }
        let gain = 0.25 + (track_index % 5) as f32 * 0.125;
        engine_tracks.push(Track::new(clips, gain, false)?);
    }
    let frames = clips_per_track as u64 * STRIDE_FRAMES;
    Ok((Project::new(48_000, engine_tracks)?, frames))
}

fn digest(hash: u64, frames: &[Frame]) -> u64 {
    frames.iter().fold(hash, |hash, frame| {
        let left = u64::from(frame[0].to_bits());
        let right = u64::from(frame[1].to_bits());
        (hash ^ left).wrapping_mul(0x100_0000_01b3) ^ right
    })
}

fn run(label: &str, tracks: usize, clips: usize) -> Result<(), hex_engine::Invalid> {
    let (project, frames) = build(tracks, clips)?;
    let blocks = frames.div_ceil(BLOCK_FRAMES as u64) as usize;
    let mut samples = Vec::with_capacity(blocks * PASSES);
    let mut output = vec![[0.0; 2]; BLOCK_FRAMES];
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let mut renderer = Renderer::new(&project);
    renderer.play();
    // One warm-up pass, then timed passes over the whole arrangement.
    for pass in 0..=PASSES {
        renderer.seek(0);
        for _ in 0..blocks {
            let start = Instant::now();
            renderer.render(black_box(&mut output))?;
            let elapsed = start.elapsed().as_nanos();
            if pass > 0 {
                samples.push(elapsed);
            } else {
                hash = digest(hash, &output);
            }
        }
    }
    samples.sort_unstable();
    let at = |q: usize| samples[(samples.len() - 1) * q / 100] as f64 / 1_000.0;
    println!(
        "{label}: {tracks} tracks × {clips} clips, {blocks} blocks of {BLOCK_FRAMES} frames × {PASSES} passes: p50={:.2}µs p95={:.2}µs max={:.2}µs digest={hash:016x}",
        at(50),
        at(95),
        at(100),
    );
    Ok(())
}

fn main() -> Result<(), hex_engine::Invalid> {
    println!(
        "release in-memory Renderer, {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    run("wide", 100, 1_000)?;
    run("dense", 1, 16_384)?;
    Ok(())
}
