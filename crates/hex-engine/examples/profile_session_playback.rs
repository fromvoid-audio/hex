//! Canonical mixed-route kernel benchmark; no device, UI or plug-in hosting.
mod support;
use hex_engine::{
    Frame, LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    playback::{Playback, PlaybackAction, PlaybackCommand},
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
use support::session_fixture::{self, RATE, kernel};

const BLOCK: usize = 128;
const RUNS: usize = 4_000;

enum ProfileRuntime {
    Kernel(Box<SessionPlayback>),
    Prepared(Playback),
}

impl ProfileRuntime {
    fn new(revision: Arc<PreparedRevision>, wrapper: bool) -> Result<Self, Box<dyn Error>> {
        if !wrapper {
            return Ok(Self::Kernel(Box::new(kernel(revision)?)));
        }
        let scene = revision
            .session()
            .scenes()
            .first()
            .ok_or("missing scene")?
            .id;
        let mut playback = Playback::try_from_prepared_with_session(revision)?;
        playback
            .try_apply_action(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .map_err(|error| error.message())?;
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .map_err(|error| error.message())?;
        Ok(Self::Prepared(playback))
    }

    fn render(&mut self, output: &mut [Frame]) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Kernel(runtime) => runtime.render(output)?,
            Self::Prepared(runtime) => runtime.render(output)?,
        }
        Ok(())
    }

    fn performance_frame(&self) -> u64 {
        match self {
            Self::Kernel(runtime) => runtime.performance_frame(),
            Self::Prepared(runtime) => runtime.performance_frame(),
        }
    }

    fn arrangement_frame(&self) -> u64 {
        match self {
            Self::Kernel(runtime) => runtime.arrangement_frame(),
            Self::Prepared(runtime) => runtime.position(),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut loop_tempo = false;
    let mut wrapper = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--loop-tempo" if !loop_tempo => loop_tempo = true,
            "--playback-wrapper" if !wrapper => wrapper = true,
            _ => {
                return Err(
                    "usage: profile_session_playback [--loop-tempo] [--playback-wrapper]".into(),
                );
            }
        }
    }
    let loop_frames = if loop_tempo { 120_000 } else { 72_000 };
    let project = session_fixture::project(loop_tempo)?;
    let oracle = session_fixture::finite_oracle(&project, loop_tempo)?;
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        hex_engine::SharedSineTable::new_on_control(),
        None,
    )?);
    let mut whole = ProfileRuntime::new(Arc::clone(&revision), wrapper)?;
    let mut split = ProfileRuntime::new(Arc::clone(&revision), wrapper)?;
    let mut expected = vec![[0.0; 2]; if loop_tempo { 260_000 } else { 160_000 }];
    let mut actual = expected.clone();
    whole.render(&mut expected)?;
    let mut reference = vec![[0.0; 2]; expected.len()];
    {
        let mut finite = hex_engine::Renderer::new(&oracle);
        finite.play();
        finite.render(&mut reference)?;
    }
    if reference != expected {
        return Err("mixed Session kernel differs from independent finite compiler oracle".into());
    }
    for chunk in actual.chunks_mut(31) {
        split.render(chunk)?;
    }
    if actual != expected
        || whole.performance_frame() != split.performance_frame()
        || whole.arrangement_frame() != split.arrangement_frame()
    {
        return Err("mixed Session kernel partition mismatch".into());
    }
    drop((whole, split, oracle, reference, expected, actual));

    let mut playback = ProfileRuntime::new(revision, wrapper)?;
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..100 {
        playback.render(&mut output)?;
    }
    let mut timings = Vec::with_capacity(RUNS);
    let mut checksum = 0_u64;
    for _ in 0..RUNS {
        let start = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(start.elapsed().as_nanos());
        for frame in black_box(&output) {
            checksum = checksum.wrapping_add(u64::from(frame[0].to_bits()));
            checksum = checksum.wrapping_add(u64::from(frame[1].to_bits()));
        }
    }
    timings.sort_unstable();
    println!(
        "mixed canonical kernel: tracks=100 arrangement_routes=50 session_routes=50 voices_per_track=4_or_8 block=128 rate=48000 loop_frames={loop_frames} loop_tempo={loop_tempo} playback_wrapper={wrapper} warmups=100 samples={RUNS}"
    );
    println!(
        "p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} above_period={} checksum={checksum} performance_frame={} arrangement_frame={}",
        timings[RUNS / 2] as f64 / 1_000.0,
        timings[(RUNS * 95).div_ceil(100) - 1] as f64 / 1_000.0,
        timings[(RUNS * 99).div_ceil(100) - 1] as f64 / 1_000.0,
        timings[RUNS - 1] as f64 / 1_000.0,
        timings
            .iter()
            .filter(|&&ns| ns * u128::from(RATE) > BLOCK as u128 * 1_000_000_000)
            .count(),
        playback.performance_frame(),
        playback.arrangement_frame(),
    );
    println!(
        "setup, partition comparison and checksums outside timings; finite original sine sources, no native callback, UI, devices, revision handoff or sustained hardware stress"
    );
    Ok(())
}
