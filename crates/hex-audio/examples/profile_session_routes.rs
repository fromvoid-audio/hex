//! Prepared callback control/snapshot benchmark; no CPAL stream or device.
use hex_audio::prepared::{self, SESSION_ROUTE_CAPACITY, SessionRouteEntry, SessionRouteOwner};
use hex_engine::{
    LaunchQuantization, SessionPlayback, SessionPlaybackCommand, SineTable, playback::Playback,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const WARMUPS: usize = 100;
const SAMPLES: usize = 4_000;
// The unchanged path is below the platform clock's single-call resolution.
const UNCHANGED_BATCH: usize = 64;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let tracks = match arguments.as_slice() {
        [] => 100,
        [count] => count.parse::<usize>()?,
        _ => return Err("usage: profile_session_routes [track count]".into()),
    };
    if tracks == 0 || tracks > SESSION_ROUTE_CAPACITY {
        return Err("track count must be between 1 and 256".into());
    }
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let scene = match editor.execute(Command::AddScene {
        name: "scene".into(),
    })? {
        Created::Scene(scene) => scene,
        _ => return Err("scene creation returned another object".into()),
    };
    for index in 0..tracks {
        let track = match editor.execute(Command::AddMidiTrack {
            name: format!("keys {index}"),
        })? {
            Created::Track(track) => track,
            _ => return Err("track creation returned another object".into()),
        };
        let clip = match editor.execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: format!("clip {index}"),
            source_length: BeatTime::from_quarters(4)?,
        })? {
            Created::SessionClip(clip) => clip,
            _ => return Err("clip creation returned another object".into()),
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4)?,
            pitch: 36 + (index % 48) as u8,
            channel: 0,
            velocity: 63,
        })?;
    }
    let revision = Arc::new(hex_engine::PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let mut oracle = SessionPlayback::new(Arc::clone(&revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|error| error.reason)?;
    let launch = SessionPlaybackCommand::LaunchScene {
        scene,
        quantization: LaunchQuantization::Immediate,
    };
    control
        .launch_and_play_with_ticket(launch)
        .map_err(|_| "launch queue full")?;
    oracle.apply(launch)?;
    oracle.apply(SessionPlaybackCommand::Play)?;
    callback.begin_block();

    let mut unchanged = Vec::with_capacity(SAMPLES);
    let mut changed = Vec::with_capacity(SAMPLES);
    let mut reads = Vec::with_capacity(SAMPLES);
    let mut routes = vec![SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
    let mut checksum = 0_u64;
    for updating in [false, true] {
        for index in 0..WARMUPS + SAMPLES {
            if updating {
                let command = if index & 1 == 0 {
                    SessionPlaybackCommand::StopAllClips {
                        quantization: LaunchQuantization::Immediate,
                    }
                } else {
                    launch
                };
                control
                    .session_command_with_ticket(command)
                    .map_err(|_| "route command queue full")?;
                oracle.apply(command)?;
            }
            let start = Instant::now();
            let repeats = if updating { 1 } else { UNCHANGED_BATCH };
            for _ in 0..repeats {
                callback.begin_block();
            }
            let elapsed = start.elapsed().as_nanos() / repeats as u128;
            if index >= WARMUPS {
                if updating {
                    changed.push(elapsed);
                } else {
                    unchanged.push(elapsed);
                }
            }
            if control.status.rejected_action().is_some() {
                return Err("benchmark action was rejected".into());
            }
            let start = Instant::now();
            let snapshot = control
                .status
                .session_routes(black_box(&mut routes))
                .ok_or("uncontended route snapshot unavailable")?;
            let elapsed = start.elapsed().as_nanos();
            if updating && index >= WARMUPS {
                reads.push(elapsed);
            }
            if snapshot.count != tracks || snapshot.audio_revision != 1 {
                return Err("snapshot revision or count mismatch".into());
            }
            for (slot, route) in routes.iter().take(tracks).enumerate() {
                let expected = oracle.route_status(slot).ok_or("oracle route missing")?;
                let owner = match expected.owner {
                    hex_engine::SessionPlaybackOwner::Arrangement => SessionRouteOwner::Arrangement,
                    hex_engine::SessionPlaybackOwner::Session { clip } => {
                        SessionRouteOwner::Session {
                            clip_id: clip.value(),
                        }
                    }
                    hex_engine::SessionPlaybackOwner::Silence => SessionRouteOwner::Silence,
                };
                if route.track_id != expected.track.value()
                    || route.owner != owner
                    || route.pending.is_some()
                {
                    return Err("snapshot route mismatch".into());
                }
                checksum = checksum.wrapping_mul(31).wrapping_add(route.track_id);
            }
            let mut actual = [[0.0; 2]; BLOCK];
            let mut expected = actual;
            callback.render(&mut actual)?;
            oracle.render(&mut expected)?;
            if actual != expected {
                return Err("callback PCM differs from headless kernel".into());
            }
            black_box(actual);
        }
    }
    println!(
        "prepared callback route telemetry: tracks={tracks} block={BLOCK} rate={RATE} warmups={WARMUPS} samples={SAMPLES} unchanged_batch={UNCHANGED_BATCH} checksum={checksum}"
    );
    report("unchanged_begin_block", &mut unchanged)?;
    report("changed_begin_block", &mut changed)?;
    report("snapshot_read", &mut reads)?;
    println!(
        "changed control timing includes route validation/release capture/snapshot writes; render, queue submission and PCM/route checks outside timing; no CPAL/UI/device/plugin or sustained-deadline claim"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
    Ok(())
}

fn report(name: &str, samples: &mut [u128]) -> Result<(), Box<dyn Error>> {
    samples.sort_unstable();
    let last = samples.len().checked_sub(1).ok_or("empty timing samples")?;
    let at =
        |percent: usize| samples.get(last * percent / 100).copied().unwrap_or(0) as f64 / 1_000.0;
    let period_ns = BLOCK as u128 * 1_000_000_000 / u128::from(RATE);
    let above_period = samples.iter().filter(|value| **value > period_ns).count();
    println!(
        "{name}: p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} above_period={above_period}",
        at(50),
        at(95),
        at(99),
        at(100)
    );
    Ok(())
}
