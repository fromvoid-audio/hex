//! Headless detached effect-tail handoff profile. No native callback or device deadline.
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const TAIL_FRAMES: usize = RATE as usize;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn fixture(
    tracks: usize,
    effects: usize,
) -> Result<(Project, Project, BTreeMap<hex_project::AssetId, Audio>)> {
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let Created::Asset(asset) = editor.execute(Command::AddAsset {
        path: "memory/tail-profile.wav".into(),
        sample_rate: RATE,
        frames: 32,
    })?
    else {
        return Err("missing tail asset".into());
    };
    let state = UTILITY_DEFINITION.patch_parameter(
        &UTILITY_DEFINITION.default_state()?,
        parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    )?;
    for i in 0..tracks {
        let Created::Track(track) = editor.execute(Command::AddTrack {
            name: format!("tail {i}"),
        })?
        else {
            return Err("missing tail track".into());
        };
        editor.execute(Command::Gain { track, gain: 0.01 })?;
        editor.execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
        })?;
        for _ in 0..effects {
            editor.execute(Command::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            })?;
        }
    }
    let old = editor.project().clone();
    let mut snapshot = old.snapshot();
    for track in &mut snapshot.tracks {
        for device in &mut track.devices {
            device.bypassed = true;
        }
    }
    let candidate = Project::from_snapshot(snapshot)?;
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.3, -0.1]; 32])?)]);
    Ok((old, candidate, media))
}
fn prepare(
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
fn started(revision: Arc<PreparedRevision>) -> Result<SessionPlayback> {
    let mut playback = SessionPlayback::new(revision)?;
    playback.apply(SessionPlaybackCommand::Play)?;
    let mut prefix = [[0.0; 2]; 32];
    playback.render(&mut prefix)?;
    if !prefix.iter().flatten().any(|sample| sample.abs() > 1e-6) {
        return Err("silent tail prefix".into());
    }
    Ok(playback)
}
fn run(tracks: usize, effects: usize, block: usize, repetitions: usize) -> Result<()> {
    let (old, candidate, media) = fixture(tracks, effects)?;
    let table = Arc::new(SineTable::new());
    let old_revision = match prepare(&old, &media, &table, None) {
        Ok(value) => value,
        Err(error)
            if tracks * effects > 256
                && error.to_string() == "stateful device runtime exceeds its bounded budget" =>
        {
            println!(
                "tracks={tracks} effects_per_track={effects} processors={} admission=refused error={error}",
                tracks * effects
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let next_revision = prepare(&candidate, &media, &table, Some(old_revision.session()))?;
    let mut reference = started(Arc::clone(&old_revision))?;
    let mut expected = vec![[0.0; 2]; TAIL_FRAMES];
    for chunk in expected.chunks_mut(block) {
        reference.render(chunk)?;
    }
    if !expected.iter().flatten().any(|sample| sample.abs() > 1e-6) {
        return Err("tail oracle is silent".into());
    }
    let final_peak = expected[TAIL_FRAMES - block..]
        .iter()
        .flatten()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if final_peak > 1.0e-6 {
        return Err("tail remains audible at the end of the oracle window".into());
    }
    let mut handoffs = Vec::with_capacity(repetitions);
    let mut renders = Vec::with_capacity(repetitions);
    let mut combined = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let mut previous = started(Arc::clone(&old_revision))?;
        let mut next = SessionPlayback::new(Arc::clone(&next_revision))?;
        let start = Instant::now();
        next.try_inherit_state_from(black_box(&mut previous))?;
        let handoff = start.elapsed().as_nanos();
        let mut output = vec![[0.0; 2]; block];
        let start = Instant::now();
        next.render(black_box(&mut output))?;
        let render = start.elapsed().as_nanos();
        if output != expected[..block] {
            return Err("detached tail differs from continuous old-chain output".into());
        }
        for expected_chunk in expected[block..].chunks(block) {
            let frames = expected_chunk.len();
            next.render(&mut output[..frames])?;
            if output[..frames] != *expected_chunk {
                return Err("detached tail differs from continuous old-chain output".into());
            }
        }
        handoffs.push(handoff);
        renders.push(render);
        combined.push(handoff + render);
        // All owners and buffers are reclaimed after both timed calls.
    }
    let late = combined
        .iter()
        .filter(|&&ns| ns * u128::from(RATE) > block as u128 * 1_000_000_000)
        .count();
    handoffs.sort_unstable();
    renders.sort_unstable();
    combined.sort_unstable();
    let p = |v: &[u128], n: usize| v[(v.len() - 1) * n / 100] as f64 / 1000.0;
    let mut digest = 0xcbf29ce484222325_u64;
    for sample in expected.iter().flatten() {
        digest = (digest ^ u64::from(sample.to_bits())).wrapping_mul(0x100000001b3);
    }
    println!(
        "tracks={tracks} effects_per_track={effects} processors={} rate={RATE} block={block} repetitions={repetitions} oracle_frames={TAIL_FRAMES} final_peak={final_peak:.9} tail_digest={digest:016x} handoff_p99_us={:.3} render_p99_us={:.3} combined_p99_us={:.3} combined_max_us={:.3} combined_beyond_period={late}",
        tracks * effects,
        p(&handoffs, 99),
        p(&renders, 99),
        p(&combined, 99),
        p(&combined, 100)
    );
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: profile_stateful_tails <effects:1|4|16> <block:128|512>".into());
    }
    let effects = args[0].parse()?;
    let block = args[1].parse()?;
    if ![1, 4, 16].contains(&effects) || ![128, 512].contains(&block) {
        return Err("profile arguments out of bounds".into());
    }
    println!(
        "headless 32-frame resident source; bypass detaches active DC filters; one-second continuous old-chain PCM reference; preparation and owner drops excluded; no native deadline"
    );
    run(100, effects, block, 100)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_tail_matches_continuous_old_chain() -> Result<()> {
        run(2, 1, 128, 2)
    }
}
