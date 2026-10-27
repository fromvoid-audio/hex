//! Sustained mixed Arrangement playback with stateful effects and identical-revision
//! handoffs. This is a headless timing fixture, not a physical device deadline.
#[allow(dead_code)]
#[path = "support/operator_feedback_fixture.rs"]
mod operator_fixture;

use hex_devices::{compressor, gate, saturator};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipGain, ClipSnapshot, DeviceSnapshot, IDENTITY_CONVERSION,
    Project, TrackContentSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const SOURCE: usize = 512;
const TRACKS: usize = 100;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn fixture(
    tracks: usize,
    devices: usize,
) -> Result<(Project, BTreeMap<hex_project::AssetId, Audio>)> {
    let source = operator_fixture::project_with_operators(tracks, 4, 0.24, 0.82, true)?;
    let mut snapshot = source.project.snapshot();
    let asset = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "memory/stateful-mixed.wav".into(),
        source_sample_rate: RATE,
        source_frames: SOURCE as u64,
        playback_frames: SOURCE as u64,
        conversion_version: IDENTITY_CONVERSION,
    });
    let states = [
        gate::GATE_DEFINITION.default_state()?,
        compressor::COMPRESSOR_DEFINITION.default_state()?,
        saturator::SATURATOR_DEFINITION.default_state()?,
    ];
    for (index, track) in snapshot.tracks.iter_mut().enumerate() {
        track.gain = 0.01;
        if index % 2 == 0 {
            let clip = snapshot.next_id;
            snapshot.next_id += 1;
            track.instrument = None;
            track.instrument_position = None;
            track.content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "mixed audio".into(),
                flavor: Default::default(),
                edit: None,
                gain: ClipGain::UNITY,
                id: clip,
                asset,
                start: 0,
                duration: u64::from(RATE) * 140,
                source_offset: (index % 16) as u64,
                source_length: SOURCE as u64 - (index % 16) as u64,
                source_phase: 0,
                looping: true,
                fade: ClipFade::neutral(u64::from(RATE) * 140)?,
            }]);
        } else if let TrackContentSnapshot::Instrument(clips) = &mut track.content {
            let length = BeatTime::from_quarters(256)?.raw();
            clips[0].length_raw = length;
            clips[0].source_length_raw = length;
            clips[0].loop_end_raw = length;
            for note in &mut clips[0].notes {
                note.duration_raw = BeatTime::from_quarters(240)?.raw();
            }
        }
        for device in 0..devices {
            track.devices.push(DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state: states[device % states.len()].clone(),
            });
            snapshot.next_id += 1;
        }
    }
    let audio_tracks: Vec<_> = snapshot
        .tracks
        .iter()
        .step_by(2)
        .map(|track| track.id)
        .collect();
    snapshot
        .native_sources
        .retain(|source| !source.receiver.is_some_and(|id| audio_tracks.contains(&id)));
    let project = Project::from_snapshot(snapshot)?;
    let asset_id = project.assets().next().ok_or("missing fixture asset")?.id();
    let samples = (0..SOURCE)
        .map(|i| {
            let level = if i < SOURCE / 2 { 0.2 } else { 0.002 };
            let sign = if i % 32 < 16 { 1.0 } else { -1.0 };
            [level * sign, -level * sign * 0.65]
        })
        .collect();
    Ok((
        project,
        BTreeMap::from([(asset_id, Audio::new(RATE, samples)?)]),
    ))
}

fn revision(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    table: &Arc<SineTable>,
    previous: Option<&hex_engine::PreparedSessionInventory>,
) -> Result<Arc<PreparedRevision>> {
    Ok(Arc::new(PreparedRevision::prepare(
        project,
        media,
        Arc::clone(table),
        previous,
    )?))
}
fn playback(revision: Arc<PreparedRevision>) -> Result<SessionPlayback> {
    let mut result = SessionPlayback::new(revision)?;
    result.apply(SessionPlaybackCommand::Play)?;
    Ok(result)
}
fn digest(hash: &mut u64, frames: &[[f32; 2]]) {
    for sample in frames.iter().flatten() {
        *hash ^= u64::from(sample.to_bits());
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}
// Independent track composition: compile each original track as a separate
// project and add its entire output in the original mixer order. This exercises
// the same device implementations but not the shared 100-track mixing graph.
fn composition_oracle(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    table: &Arc<SineTable>,
    block: usize,
    total: usize,
) -> Result<Vec<[f32; 2]>> {
    let snapshot = project.snapshot();
    let mut sum = vec![[0.0_f32; 2]; total];
    let mut output = vec![[0.0_f32; 2]; block];
    for track in &snapshot.tracks {
        let mut isolated = snapshot.clone();
        isolated.tracks.retain(|candidate| candidate.id == track.id);
        isolated
            .native_sources
            .retain(|source| source.receiver == Some(track.id));
        let isolated = Project::from_snapshot(isolated)?;
        let prepared = revision(&isolated, media, table, None)?;
        let mut playback = playback(prepared)?;
        for (offset, chunk) in sum.chunks_mut(block).enumerate() {
            playback.render(&mut output[..chunk.len()])?;
            for (mixed, source) in chunk.iter_mut().zip(&output) {
                for channel in 0..2 {
                    if !source[channel].is_finite() {
                        return Err(format!(
                            "nonfinite isolated PCM at track {} block {offset}",
                            track.id
                        )
                        .into());
                    }
                    mixed[channel] += source[channel];
                }
            }
        }
    }
    Ok(sum)
}
fn run(
    tracks: usize,
    devices: usize,
    block: usize,
    seconds: usize,
    composition: bool,
) -> Result<()> {
    let (project, media) = fixture(tracks, devices)?;
    let table = Arc::new(SineTable::new());
    let initial = match revision(&project, &media, &table, None) {
        Ok(value) => value,
        Err(error)
            if devices * tracks > 256
                && error.to_string() == "stateful device runtime exceeds its bounded budget" =>
        {
            println!(
                "tracks={tracks} devices_per_track={devices} requested_effects={} requested_mutable_instruments={} admission=refused error={error}",
                tracks * devices,
                tracks / 2
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let total = seconds * RATE as usize;
    let composed = if composition {
        Some(composition_oracle(&project, &media, &table, block, total)?)
    } else {
        None
    };
    let mut composition_max_error = 0.0_f32;
    let mut current_revision = Arc::clone(&initial);
    let mut actual = playback(Arc::clone(&initial))?;
    let mut reference = playback(initial)?;
    let mut a = vec![[0.0; 2]; block];
    let mut b = a.clone();
    let mut timings = Vec::new();
    let mut handoffs = Vec::new();
    let mut handoff_plus_render = Vec::new();
    let mut pending_handoff = None;
    let mut hash = 0xcbf29ce484222325;
    let mut frame = 0;
    let mut next_handoff = RATE as usize * 10;
    let mut peak = 0.0_f32;
    let mut window_peak = 0.0_f32;
    let mut first_audible_frame = None;
    let mut first_render_ns = 0;
    while frame < total {
        if frame >= next_handoff {
            let prepared = revision(&project, &media, &table, Some(current_revision.session()))?;
            let mut candidate = SessionPlayback::new(Arc::clone(&prepared))?;
            let start = Instant::now();
            candidate.try_inherit_state_from(&mut actual)?;
            let elapsed = start.elapsed().as_nanos();
            handoffs.push(elapsed);
            pending_handoff = Some(elapsed);
            // Old owners are retired outside the timed handoff and render.
            actual = candidate;
            current_revision = prepared;
            next_handoff += RATE as usize * 10;
        }
        let count = block.min(total - frame);
        let start = Instant::now();
        actual.render(black_box(&mut a[..count]))?;
        let elapsed = start.elapsed().as_nanos();
        if frame == 0 {
            first_render_ns = elapsed;
        }
        if let Some(handoff) = pending_handoff.take() {
            handoff_plus_render.push((count, handoff + elapsed));
        }
        timings.push((count, elapsed));
        reference.render(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Err(format!("continuous PCM mismatch at frame {frame}").into());
        }
        if let Some(composed) = &composed {
            for (actual, expected) in a[..count].iter().zip(&composed[frame..frame + count]) {
                for channel in 0..2 {
                    let error = (actual[channel] - expected[channel]).abs();
                    if !error.is_finite() || error > 2.0e-6 {
                        return Err(format!(
                            "isolated-track composition mismatch at frame {frame} error={error}"
                        )
                        .into());
                    }
                    composition_max_error = composition_max_error.max(error);
                }
            }
        }
        for (offset, channels) in a[..count].iter().enumerate() {
            for sample in channels {
                if !sample.is_finite() {
                    return Err("nonfinite output".into());
                }
                peak = peak.max(sample.abs());
                window_peak = window_peak.max(sample.abs());
                if first_audible_frame.is_none() && sample.abs() > 1.0e-6 {
                    first_audible_frame = Some(frame + offset);
                }
            }
            if (frame + offset + 1) % RATE as usize == 0 {
                if window_peak < 1.0e-6 {
                    return Err("silent one-second window".into());
                }
                window_peak = 0.0;
            }
        }
        digest(&mut hash, &a[..count]);
        frame += count;
    }
    if peak < 1.0e-6 {
        return Err("silent fixture".into());
    }
    let late = timings
        .iter()
        .filter(|&&(frames, ns)| ns * u128::from(RATE) > frames as u128 * 1_000_000_000)
        .count();
    let transition_late = handoff_plus_render
        .iter()
        .filter(|&&(frames, ns)| ns * u128::from(RATE) > frames as u128 * 1_000_000_000)
        .count();
    let transition_max = handoff_plus_render
        .iter()
        .map(|&(_, ns)| ns)
        .max()
        .unwrap_or(0);
    timings.sort_unstable_by_key(|&(_, ns)| ns);
    handoffs.sort_unstable();
    let p = |pct: usize| timings[(timings.len() - 1) * pct / 100].1 as f64 / 1000.0;
    println!(
        "tracks={tracks} audio_tracks={} operator_tracks={} devices_per_track={devices} effect_instances={} mutable_instruments={} total_stateful_instances={} admission=accepted rate={RATE} block={block} seconds={seconds} frames={frame} digest={hash:016x} peak={peak:.8} render_p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} beyond_actual_period={late} first_audible_frame={} first_render_us={:.3} handoffs={} handoff_max_us={:.3} handoff_plus_render_max_us={:.3} transition_beyond_period={transition_late} isolated_track_composition={composition} composition_max_error={composition_max_error:.9}",
        tracks.div_ceil(2),
        tracks / 2,
        tracks * devices,
        tracks / 2,
        tracks * devices + tracks / 2,
        p(50),
        p(95),
        p(99),
        p(100),
        first_audible_frame.ok_or("no audible frame")?,
        first_render_ns as f64 / 1000.0,
        handoffs.len(),
        handoffs.last().copied().unwrap_or(0) as f64 / 1000.0,
        transition_max as f64 / 1000.0
    );
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 && !(args.len() == 4 && args[3] == "--composition") {
        return Err(
            "usage: profile_stateful_mixed <devices:1|4|16> <block:128|512> <seconds:1..60> [--composition]".into(),
        );
    }
    let devices = args[0].parse()?;
    let block = args[1].parse()?;
    let seconds = args[2].parse()?;
    if ![1, 4, 16].contains(&devices)
        || ![128, 512].contains(&block)
        || !(1..=60).contains(&seconds)
    {
        return Err("profile arguments out of bounds".into());
    }
    println!(
        "headless mixed resident audio and four-voice all-operator arrangement; identical revision handoffs; continuous playback PCM reference; uncontrolled host load; no physical callback or first-audible measurement"
    );
    run(TRACKS, devices, block, seconds, args.len() == 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_fixture_handoff_preserves_continuous_pcm() -> Result<()> {
        let (project, media) = fixture(2, 1)?;
        let table = Arc::new(SineTable::new());
        let initial = revision(&project, &media, &table, None)?;
        let mut original = playback(Arc::clone(&initial))?;
        let mut reference = playback(Arc::clone(&initial))?;
        let mut a = [[0.0; 2]; 128];
        let mut b = a;
        for _ in 0..4 {
            original.render(&mut a)?;
            reference.render(&mut b)?;
            assert_eq!(a, b);
        }
        let prepared = revision(&project, &media, &table, Some(initial.session()))?;
        let mut candidate = SessionPlayback::new(prepared)?;
        candidate.try_inherit_state_from(&mut original)?;
        for _ in 0..4 {
            candidate.render(&mut a)?;
            reference.render(&mut b)?;
            assert_eq!(a, b);
        }
        assert!(a.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        Ok(())
    }

    #[test]
    fn isolated_track_composition_matches_and_detects_missing_track() -> Result<()> {
        let (project, media) = fixture(2, 1)?;
        let table = Arc::new(SineTable::new());
        let composed = composition_oracle(&project, &media, &table, 128, 1024)?;
        let mut full = playback(revision(&project, &media, &table, None)?)?;
        let mut actual = [[0.0_f32; 2]; 1024];
        for chunk in actual.chunks_mut(128) {
            full.render(chunk)?;
        }
        assert!(
            actual
                .iter()
                .zip(&composed)
                .all(|(a, b)| (0..2).all(|c| (a[c] - b[c]).abs() <= 2.0e-6))
        );
        let mut missing = project.snapshot();
        missing.tracks.truncate(1);
        missing.native_sources.clear();
        let missing = Project::from_snapshot(missing)?;
        let incomplete = composition_oracle(&missing, &media, &table, 128, 1024)?;
        assert!(
            actual
                .iter()
                .zip(&incomplete)
                .any(|(a, b)| (0..2).any(|c| (a[c] - b[c]).abs() > 2.0e-6))
        );
        Ok(())
    }

    #[test]
    fn dense_fixture_refuses_the_existing_effect_budget() -> Result<()> {
        let (project, media) = fixture(100, 4)?;
        let table = Arc::new(SineTable::new());
        let error = match revision(&project, &media, &table, None) {
            Ok(_) => return Err("400 stateful effects unexpectedly admitted".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "stateful device runtime exceeds its bounded budget"
        );
        Ok(())
    }
}
