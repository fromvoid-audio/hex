//! Dense mixed playback using separate one-track owners. This does not admit a
//! unified 100-track graph or establish native callback deadlines.
#[allow(dead_code)]
#[path = "profile_stateful_mixed.rs"]
mod mixed;

use hex_engine::{PreparedRevision, SessionPlayback, SineTable};
use hex_project::Project;
use std::{error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: usize = 48_000;
// Equal source position at each supported block size for cross-block oracles.
const WARMUP_FRAMES: usize = 51_200;
const TOLERANCE: f32 = 2.0e-6;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Owner {
    project: Project,
    revision: Arc<PreparedRevision>,
    actual: SessionPlayback,
    reference: SessionPlayback,
}

fn compare(actual: &[[f32; 2]], reference: &[[f32; 2]]) -> Result<f32> {
    if actual.len() != reference.len() {
        return Err("isolated mixed frame count mismatch".into());
    }
    let mut max_error = 0.0_f32;
    for (actual, reference) in actual.iter().zip(reference) {
        for channel in 0..2 {
            let error = (actual[channel] - reference[channel]).abs();
            if !error.is_finite() || error > TOLERANCE {
                return Err(format!("isolated mixed mismatch: {error}").into());
            }
            max_error = max_error.max(error);
        }
    }
    Ok(max_error)
}

fn run(tracks: usize, devices: usize, block: usize, seconds: usize) -> Result<u64> {
    if !(1..=100).contains(&tracks)
        || ![1, 4, 16].contains(&devices)
        || ![128, 512].contains(&block)
        || !(1..=60).contains(&seconds)
    {
        return Err("isolated mixed profile bounds exceeded".into());
    }
    let (project, media) = mixed::fixture(tracks, devices)?;
    let snapshot = project.snapshot();
    let table = Arc::new(SineTable::new());
    let mut owners = Vec::with_capacity(tracks);
    for track in &snapshot.tracks {
        let mut isolated = snapshot.clone();
        isolated.tracks.retain(|candidate| candidate.id == track.id);
        isolated
            .native_sources
            .retain(|source| source.receiver == Some(track.id));
        let project = Project::from_snapshot(isolated)?;
        let revision = mixed::revision(&project, &media, &table, None)?;
        owners.push(Owner {
            project,
            actual: mixed::playback(Arc::clone(&revision))?,
            reference: mixed::playback(Arc::clone(&revision))?,
            revision,
        });
    }
    let total = seconds * RATE;
    let mut sum = vec![[0.0_f32; 2]; block];
    let mut expected = sum.clone();
    let mut scratch = sum.clone();
    let mut timings = Vec::with_capacity(total.div_ceil(block));
    let mut handoffs = Vec::new();
    let mut handoff_renders = Vec::new();
    let mut pending_handoff = None;
    let mut digest = 0xcbf2_9ce4_8422_2325;
    let mut max_error = 0.0_f32;
    let mut window_peak = 0.0_f32;
    let mut windows = 0;
    let mut frame = 0;
    let mut next_handoff = RATE * 10;
    let warmup_blocks = WARMUP_FRAMES / block;
    for iteration in 0..warmup_blocks + total.div_ceil(block) {
        let warmup = iteration < warmup_blocks;
        let frames = if warmup {
            block
        } else {
            block.min(total - frame)
        };
        if !warmup && frame >= next_handoff {
            let mut candidates = Vec::with_capacity(owners.len());
            for owner in &owners {
                let revision = mixed::revision(
                    &owner.project,
                    &media,
                    &table,
                    Some(owner.revision.session()),
                )?;
                let candidate = SessionPlayback::new(Arc::clone(&revision))?;
                candidates.push((revision, candidate));
            }
            let start = Instant::now();
            for (owner, (revision, candidate)) in owners.iter_mut().zip(&mut candidates) {
                candidate.try_inherit_state_from(&mut owner.actual)?;
                std::mem::swap(&mut owner.actual, candidate);
                std::mem::swap(&mut owner.revision, revision);
            }
            let elapsed = start.elapsed().as_nanos();
            handoffs.push(elapsed);
            pending_handoff = Some(elapsed);
            drop(candidates);
            next_handoff += RATE * 10;
        }
        sum[..frames].fill([0.0; 2]);
        let start = Instant::now();
        for owner in &mut owners {
            owner.actual.render(black_box(&mut scratch[..frames]))?;
            for (sum, output) in sum[..frames].iter_mut().zip(&scratch[..frames]) {
                sum[0] += output[0];
                sum[1] += output[1];
            }
        }
        let elapsed = start.elapsed().as_nanos();
        if let Some(handoff) = pending_handoff.take() {
            handoff_renders.push((frames, handoff + elapsed));
        }
        expected[..frames].fill([0.0; 2]);
        for owner in &mut owners {
            owner.reference.render(&mut scratch[..frames])?;
            for (sum, output) in expected[..frames].iter_mut().zip(&scratch[..frames]) {
                sum[0] += output[0];
                sum[1] += output[1];
            }
        }
        max_error = max_error.max(compare(&sum[..frames], &expected[..frames])?);
        if !warmup {
            timings.push((frames, elapsed));
            mixed::digest(&mut digest, &sum[..frames]);
            for (index, sample) in sum[..frames].iter().enumerate() {
                window_peak = window_peak.max(sample[0].abs()).max(sample[1].abs());
                if (frame + index + 1) % RATE == 0 {
                    if window_peak < 1.0e-6 {
                        return Err("silent measured second".into());
                    }
                    windows += 1;
                    window_peak = 0.0;
                }
            }
            frame += frames;
        }
    }
    if windows != seconds {
        return Err("incomplete activity windows".into());
    }
    if handoffs.len() != (seconds - 1) / 10 || handoff_renders.len() != handoffs.len() {
        return Err("incomplete handoff rounds".into());
    }
    let late = timings
        .iter()
        .filter(|&&(frames, ns)| ns * RATE as u128 > frames as u128 * 1_000_000_000)
        .count();
    let transition_late = handoff_renders
        .iter()
        .filter(|&&(frames, ns)| ns * RATE as u128 > frames as u128 * 1_000_000_000)
        .count();
    let transition_max = handoff_renders.iter().map(|&(_, ns)| ns).max().unwrap_or(0);
    timings.sort_unstable_by_key(|&(_, ns)| ns);
    let percentile = |p: usize| timings[(timings.len() - 1) * p / 100].1 as f64 / 1000.0;
    println!(
        "isolated_owners={tracks} audio_tracks={} operator_tracks={} effects_per_owner={devices} total_effects={} rate={RATE} block={block} seconds={seconds} warmup_blocks={warmup_blocks} warmup_frames={WARMUP_FRAMES} measured_blocks={} activity_windows={windows} handoff_rounds={} handoff_max_us={:.3} handoff_plus_render_max_us={:.3} transition_beyond_period={transition_late} max_reference_error={max_error:e} digest={digest:016x} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} beyond_period={late}; separate owners and continuity reference, preparation/retirement outside timing, no unified admission or native deadline",
        tracks.div_ceil(2),
        tracks / 2,
        tracks * devices,
        timings.len(),
        handoffs.len(),
        handoffs.iter().copied().max().unwrap_or(0) as f64 / 1000.0,
        transition_max as f64 / 1000.0,
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100)
    );
    Ok(digest)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "usage: profile_isolated_mixed <effects:1|4|16> <block:128|512> <seconds:1..60>".into(),
        );
    }
    run(100, args[0].parse()?, args[1].parse()?, args[2].parse()?).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_owners_retain_output_at_both_block_sizes() -> Result<()> {
        assert_eq!(run(2, 4, 128, 1)?, run(2, 4, 512, 1)?);
        assert_eq!(run(2, 4, 128, 11)?, run(2, 4, 512, 11)?);
        Ok(())
    }

    #[test]
    fn comparison_rejects_missing_or_nonfinite_output() {
        assert!(compare(&[[0.1, 0.2]], &[[0.0, 0.2]]).is_err());
        assert!(compare(&[[f32::NAN, 0.2]], &[[0.0, 0.2]]).is_err());
        assert!(compare(&[], &[[0.0, 0.2]]).is_err());
    }
}
