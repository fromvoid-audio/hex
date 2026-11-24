//! Run with `cargo run --release -p hex-engine --example profile_ringing_group`.
//! Times the first 256 frames after two sustained MIDI tracks stop.
#[allow(dead_code)]
#[path = "support/session_fixture.rs"]
mod session_fixture;

use hex_engine::{
    DeviceTypeId, LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    SineTable, default_native_device_state,
};
use hex_project::{Command, Created, Editor, TrackId, TrackSelectionIds};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const BLOCK: usize = 256;
const RUNS: usize = 40;

fn ringing(
    revision: &Arc<PreparedRevision>,
    stops: &[TrackId],
) -> Result<SessionPlayback, Box<dyn Error>> {
    let mut playback = SessionPlayback::new(Arc::clone(revision))?;
    playback.apply(SessionPlaybackCommand::Play)?;
    playback.render(&mut [[0.0; 2]; 1024])?;
    for &track in stops {
        playback.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })?;
    }
    if playback.release_voice_count() == 0 {
        return Err("fixture did not retain MIDI release voices".into());
    }
    Ok(playback)
}

fn main() -> Result<(), Box<dyn Error>> {
    let project = session_fixture::project_with_tracks(8, false)?;
    let tracks: Vec<_> = project.tracks().iter().map(|track| track.id()).collect();
    let stops = [tracks[0], tracks[4]];
    let mut editor = Editor::new(project, 4)?;
    let Created::Track(group) = editor.execute(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&tracks[..4])?,
        name: "ringing group".into(),
    })?
    else {
        return Err("missing group".into());
    };
    for (track, effect) in [(tracks[0], "hex.saturator"), (group, "hex.limiter")] {
        editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: default_native_device_state(DeviceTypeId::from_static(effect))?,
        })?;
    }
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    if revision.arrangement().main_latency_frames() == 0 {
        return Err("fixture requires upstream latency compensation".into());
    }
    let mut scalar = ringing(&revision, &stops)?;
    let voices = scalar.release_voice_count();
    let mut reference = [[0.0; 2]; BLOCK];
    for sample in reference.chunks_mut(1) {
        scalar.render(sample)?;
    }
    if !reference.iter().flatten().all(|x| x.is_finite())
        || !reference.iter().flatten().any(|x| *x != 0.0)
    {
        return Err("expected audible finite output".into());
    }
    let bits = reference.map(|frame| frame.map(f32::to_bits));
    let checksum = bits
        .iter()
        .flatten()
        .fold(0xcbf29ce484222325_u64, |hash, word| {
            (hash ^ u64::from(*word)).wrapping_mul(0x100000001b3)
        });
    let mut timings = Vec::with_capacity(RUNS);
    for run in 0..RUNS + 5 {
        let mut playback = ringing(&revision, &stops)?;
        let mut output = [[0.0; 2]; BLOCK];
        let start = Instant::now();
        playback.render(black_box(&mut output))?;
        let elapsed = start.elapsed();
        if output.map(|frame| frame.map(f32::to_bits)) != bits {
            return Err("block output differs from scalar reference".into());
        }
        if run >= 5 {
            timings.push(elapsed);
        }
    }
    timings.sort_unstable();
    println!(
        "rate={} block={BLOCK} tracks=8 grouped=4 release_voices={voices} samples={RUNS} pcm={checksum:016x} p50_us={:.2} p95_us={:.2}",
        session_fixture::RATE,
        timings[RUNS / 2 - 1].as_secs_f64() * 1e6,
        timings[(RUNS * 95).div_ceil(100) - 1].as_secs_f64() * 1e6
    );
    Ok(())
}
