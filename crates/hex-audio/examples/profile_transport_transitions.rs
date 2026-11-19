//! Headless prepared-callback transport timings; no physical device is used.
use hex_audio::prepared;
use hex_engine::{
    LaunchQuantization, PreparedRevision, SessionPlaybackCommand, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    error::Error,
    hint::black_box,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const TRACKS: usize = 100;
const WARMUPS: usize = 20;
const SAMPLES: usize = 200;
const SETTLE_BLOCKS: usize = 4;

fn main() -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let Created::Scene(scene) = editor.execute(Command::AddScene {
        name: "scene".into(),
    })?
    else {
        return Err("scene creation returned another object".into());
    };
    for index in 0..TRACKS {
        let Created::Track(track) = editor.execute(Command::AddMidiTrack {
            name: format!("keys {index}"),
        })?
        else {
            return Err("track creation returned another object".into());
        };
        editor.execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state()?,
        })?;
        let Created::SessionClip(clip) = editor.execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: format!("clip {index}"),
            source_length: BeatTime::from_quarters(4)?,
        })?
        else {
            return Err("clip creation returned another object".into());
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
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(revision)?)
        .map_err(|error| error.reason)?;
    callback.begin_block();
    let launch = SessionPlaybackCommand::LaunchScene {
        scene,
        quantization: LaunchQuantization::Immediate,
    };
    let mut timings: [Vec<u128>; 5] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
    let mut checksum = 0_u64;
    let mut output = [[0.0_f32; 2]; BLOCK];
    for cycle in 0..WARMUPS + SAMPLES {
        for (transition, samples) in timings.iter_mut().enumerate() {
            let ticket = match transition {
                0 => Some(
                    control
                        .launch_and_play_with_ticket(launch)
                        .map_err(|_| "start queue full")?,
                ),
                1 => Some(
                    control
                        .command_with_ticket(PlaybackCommand::Pause)
                        .map_err(|_| "pause queue full")?,
                ),
                2 => Some(
                    control
                        .command_with_ticket(PlaybackCommand::Play)
                        .map_err(|_| "resume queue full")?,
                ),
                3 => Some(
                    control
                        .command_with_ticket(PlaybackCommand::Seek(12_000))
                        .map_err(|_| "seek queue full")?,
                ),
                4 => {
                    control.stop();
                    None
                }
                _ => return Err("invalid transition".into()),
            };
            let start = Instant::now();
            callback.begin_block();
            callback.render(black_box(&mut output))?;
            let elapsed = start.elapsed().as_nanos();
            if output.iter().flatten().any(|sample| !sample.is_finite()) {
                return Err("non-finite transition output".into());
            }
            if cycle >= WARMUPS {
                samples.push(elapsed);
            }
            for _ in 0..SETTLE_BLOCKS {
                callback.begin_block();
                callback.render(&mut output)?;
            }
            if control.status.rejected_action().is_some() {
                return Err("transport action rejected".into());
            }
            if ticket.is_some_and(|ticket| {
                control.status.consumed_command.load(Ordering::Acquire) < ticket
            }) {
                return Err("transport action not consumed".into());
            }
            let playing = control.status.playing.load(Ordering::Acquire);
            if playing != matches!(transition, 0 | 2 | 3) {
                return Err(
                    format!("unexpected playing state after transition {transition}").into(),
                );
            }
            let position = control.status.position.load(Ordering::Acquire);
            if transition == 3
                && !(12_000..=12_000 + ((SETTLE_BLOCKS + 1) * BLOCK) as u64).contains(&position)
            {
                return Err(format!("seek did not reach its target: {position}").into());
            }
            if transition == 4 && position != 0 {
                return Err(format!("stop did not rewind: {position}").into());
            }
            if output.iter().flatten().any(|sample| !sample.is_finite()) {
                return Err("non-finite output".into());
            }
            if playing && !output.iter().flatten().any(|sample| *sample != 0.0) {
                return Err(format!("silent output after transition {transition}").into());
            }
            for frame in &output {
                checksum = checksum
                    .wrapping_mul(31)
                    .wrapping_add(u64::from(frame[0].to_bits()));
                checksum = checksum
                    .wrapping_mul(31)
                    .wrapping_add(u64::from(frame[1].to_bits()));
            }
        }
    }
    println!(
        "prepared callback transport: tracks={TRACKS} session_notes_per_track=1 rate={RATE} block={BLOCK} warmups={WARMUPS} samples={SAMPLES} settle_blocks={SETTLE_BLOCKS} checksum={checksum}"
    );
    for (name, samples) in ["start", "pause", "resume", "seek", "stop"]
        .into_iter()
        .zip(&mut timings)
    {
        samples.sort_unstable();
        let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100) - 1] as f64 / 1_000.0;
        let period = BLOCK as u128 * 1_000_000_000 / u128::from(RATE);
        println!(
            "{name}: p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} above_period={}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100),
            samples.iter().filter(|&&value| value > period).count()
        );
    }
    println!(
        "start launches the scene; stop uses the priority path. Timings include begin_block and one render; queue submission, settling, validation and checksum are outside timing; headless only, no CPAL, physical deadline, UI or listening claim"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
    Ok(())
}
