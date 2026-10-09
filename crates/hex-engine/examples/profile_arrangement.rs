//! Compare block-local clip spans with a scalar all-clips-per-frame baseline.
//! This is an in-memory renderer benchmark, not an audio-device deadline test.
use hex_engine::{Audio, Clip, Frame, Project, Renderer, Track};
use std::{hint::black_box, time::Instant};

const TRACK_COUNT: usize = 100;
const CLIPS_PER_TRACK: usize = 100;
const BLOCK_FRAMES: usize = 128;
const ITERATIONS: usize = 40;
const ARRANGEMENT_FRAMES: u64 = 500_000;

struct ReferenceClip {
    start: u64,
    duration: u64,
    looping: bool,
}

struct ReferenceTrack {
    clips: Vec<ReferenceClip>,
    source: Vec<Frame>,
    gain: f32,
    muted: bool,
}

fn build_project(active: bool) -> Result<(Project, Vec<ReferenceTrack>), hex_engine::Invalid> {
    let mut engine_tracks = Vec::with_capacity(TRACK_COUNT);
    let mut reference_tracks = Vec::with_capacity(TRACK_COUNT);
    for track_index in 0..TRACK_COUNT {
        let source = (0..64)
            .map(|frame| {
                let left = ((track_index * 17 + frame * 11) % 97) as f32 / 128.0 - 0.25;
                let right = ((track_index * 7 + frame * 19) % 89) as f32 / 128.0 - 0.2;
                [left, right]
            })
            .collect::<Vec<_>>();
        let audio = Audio::new(48_000, source.clone())?;
        let gain = 0.25 + (track_index % 5) as f32 * 0.125;
        let muted = !active && track_index % 13 == 0;
        let mut clips = Vec::with_capacity(CLIPS_PER_TRACK);
        let mut reference_clips = Vec::with_capacity(CLIPS_PER_TRACK);
        for clip_index in 0..CLIPS_PER_TRACK {
            let mut start =
                ((track_index * 1_619 + clip_index * 4_093 + track_index * clip_index * 31) as u64)
                    % ARRANGEMENT_FRAMES;
            let mut duration = match clip_index % 3 {
                0 => 16,
                1 => 32,
                _ => 64,
            };
            let mut looping = (track_index + clip_index) % 3 == 0;
            if active && clip_index == 0 {
                start = 0;
                duration = ARRANGEMENT_FRAMES;
                looping = true;
            }
            clips.push(Clip::new(audio.clone(), start, duration, looping)?);
            reference_clips.push(ReferenceClip {
                start,
                duration,
                looping,
            });
        }
        engine_tracks.push(Track::new(clips, gain, muted)?);
        reference_tracks.push(ReferenceTrack {
            clips: reference_clips,
            source,
            gain,
            muted,
        });
    }
    Ok((Project::new(48_000, engine_tracks)?, reference_tracks))
}

/// Independent scalar baseline that checks every clip for every output frame.
fn render_scalar(tracks: &[ReferenceTrack], position: u64, output: &mut [Frame]) {
    output.fill([0.0; 2]);
    for (offset, frame) in output.iter_mut().enumerate() {
        let timeline = position + offset as u64;
        for track in tracks {
            if track.muted {
                continue;
            }
            for clip in &track.clips {
                if timeline < clip.start || timeline >= clip.start + clip.duration {
                    continue;
                }
                let source_offset = timeline - clip.start;
                if !clip.looping && source_offset >= track.source.len() as u64 {
                    continue;
                }
                let sample = track.source[(source_offset % track.source.len() as u64) as usize];
                frame[0] += sample[0] * track.gain;
                frame[1] += sample[1] * track.gain;
            }
        }
    }
}

fn digest(frames: &[Frame]) -> u64 {
    frames.iter().fold(0xcbf2_9ce4_8422_2325, |hash, frame| {
        let left = u64::from(frame[0].to_bits());
        let right = u64::from(frame[1].to_bits());
        (hash ^ left).wrapping_mul(0x100_0000_01b3) ^ right
    })
}

fn report(label: &str, samples: &mut [u128]) {
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2] as f64 / 1_000.0;
    let p95 = samples[samples.len() * 95 / 100] as f64 / 1_000.0;
    let max = samples[samples.len() - 1] as f64 / 1_000.0;
    println!("{label}: p50={p50:.2}µs p95={p95:.2}µs max={max:.2}µs");
}

fn main() -> Result<(), hex_engine::Invalid> {
    let active = std::env::args()
        .skip(1)
        .any(|argument| argument == "--active");
    let (project, reference) = build_project(active)?;
    let positions = (0..ITERATIONS)
        .map(|iteration| (iteration as u64 * 104_729) % (ARRANGEMENT_FRAMES - BLOCK_FRAMES as u64))
        .collect::<Vec<_>>();

    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut optimized_output = vec![[0.0; 2]; BLOCK_FRAMES];
    let mut scalar_output = vec![[0.0; 2]; BLOCK_FRAMES];
    for position in [0, 1, 17_003, 119_991, 401_122] {
        render_scalar(&reference, position, &mut scalar_output);
        renderer.seek(position);
        renderer.render(&mut optimized_output)?;
        assert_eq!(
            optimized_output, scalar_output,
            "output mismatch at {position}"
        );
    }

    let mut scalar_times = Vec::with_capacity(ITERATIONS);
    let mut optimized_times = Vec::with_capacity(ITERATIONS);
    let mut scalar_digest = 0_u64;
    let mut optimized_digest = 0_u64;
    for &position in &positions {
        let start = Instant::now();
        render_scalar(&reference, position, black_box(&mut scalar_output));
        scalar_times.push(start.elapsed().as_nanos());
        scalar_digest ^= digest(black_box(&scalar_output));

        renderer.seek(position);
        let start = Instant::now();
        renderer.render(black_box(&mut optimized_output))?;
        optimized_times.push(start.elapsed().as_nanos());
        assert_eq!(
            optimized_output, scalar_output,
            "output mismatch at {position}"
        );
        optimized_digest ^= digest(black_box(&optimized_output));
    }
    assert_eq!(
        scalar_digest, optimized_digest,
        "benchmark output digest mismatch"
    );
    println!(
        "arrangement: {TRACK_COUNT} tracks × {CLIPS_PER_TRACK} clips, {BLOCK_FRAMES}-frame blocks, {ITERATIONS} iterations, {}, {}-{}",
        if active {
            "100 sustained active loops plus sparse clips"
        } else {
            "mostly inactive clips"
        },
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    report("scalar baseline", &mut scalar_times);
    report("block-local spans", &mut optimized_times);
    println!("matching output digest: {scalar_digest:016x}");
    Ok(())
}
