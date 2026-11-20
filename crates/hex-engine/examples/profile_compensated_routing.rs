//! Headless, reproducible compensated Track, Return, and mixed-source workloads.
//!
//! Run with `cargo run --release -p hex-engine --example profile_compensated_routing`.
//! This measures SessionPlayback, not a native callback or device deadline.
#[allow(dead_code)] // This workload reuses only the fixture's Arrangement sources.
#[path = "support/session_audio_fixture.rs"]
mod session_audio_fixture;

use hex_engine::{
    PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{BeatTime, Command, Created, Editor, TrackDeviceSlot};
use std::{error::Error, hint::black_box, sync::Arc, time::Instant};

const BLOCK: usize = 256;
const TRACKS: usize = 100;
const WARMUP: usize = 100;
const MEASURED: usize = 300;
const VERIFY_FRAMES: usize = 8_448; // Includes the first loop wrap.

#[derive(Clone, Copy)]
enum Workload {
    Track,
    Return,
    Mixed,
    MeteredNotes,
}

fn prepared(workload: Workload) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let fixture = session_audio_fixture::fixture(false)?;
    if fixture.tracks.len() != TRACKS {
        return Err("fixture track count changed".into());
    }
    let mut editor = Editor::new(fixture.project, 8)?;
    let limiter = hex_devices::limiter::DEFINITION.default_state()?;
    if matches!(workload, Workload::Return) {
        let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
            name: "profile return".into(),
        })?
        else {
            return Err("expected return track".into());
        };
        editor.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: limiter,
        })?;
        for track in &fixture.tracks {
            editor.execute(Command::SetReturnSend {
                track: *track,
                return_track: bus,
                gain: 0.125,
            })?;
        }
    } else {
        editor.execute(Command::InsertDevice {
            track: *fixture.tracks.first().ok_or("fixture has no tracks")?,
            before: None,
            state: limiter,
        })?;
    }
    if matches!(workload, Workload::Mixed | Workload::MeteredNotes) {
        // Interleave mutable instruments with already-aligned audio tracks,
        // including an instrument first in the Main mix order.
        let state = hex_devices::operator_state::OPERATOR_METADATA.default_state()?;
        for (ordinal, &before_track) in fixture.tracks.iter().step_by(25).enumerate() {
            let Created::Track(track) = editor.execute(Command::InsertTrackWithDevice {
                name: "profile operator".into(),
                before_track: Some(before_track),
                slot: TrackDeviceSlot::Instrument,
                state: state.clone(),
            })?
            else {
                return Err("expected instrument track".into());
            };
            let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4)?,
            })?
            else {
                return Err("expected MIDI clip".into());
            };
            if matches!(workload, Workload::MeteredNotes) {
                let duration = editor.project().tempo_map().frame_to_beat(192)?;
                for (note, frame) in (ordinal * 117..8_192).step_by(512).enumerate() {
                    let start = editor.project().tempo_map().frame_to_beat(frame as u64)?;
                    editor.execute(Command::AddMidiNote {
                        clip,
                        start,
                        duration,
                        pitch: 48 + ordinal as u8 + (note % 5) as u8,
                        channel: 0,
                        velocity: 96,
                    })?;
                }
            } else {
                editor.execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 48 + ordinal as u8,
                    channel: 0,
                    velocity: 96,
                })?;
            }
        }
    }
    editor.execute(Command::SetLoopRegion {
        start: 0,
        end: 8_192,
        enabled: true,
    })?;
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &fixture.media,
        Arc::new(SineTable::new()),
        None,
    )?);
    // The fixture has no Main devices, so a nonzero Main arrival is produced
    // by the Track/Return graph and enables upstream compensated routing.
    if revision.arrangement().main_latency_frames() == 0 {
        return Err("fixture did not prepare a compensated graph".into());
    }
    Ok(revision)
}

fn playback(revision: Arc<PreparedRevision>, metered: bool) -> Result<Playback, Box<dyn Error>> {
    let mut playback = Playback::try_from_prepared_with_session(revision)?;
    playback.set_device_metering(metered);
    playback.apply(PlaybackCommand::Play);
    Ok(playback)
}

fn verify(revision: &Arc<PreparedRevision>, metered: bool) -> Result<(), Box<dyn Error>> {
    let mut scalar = playback(Arc::clone(revision), metered)?;
    let mut block = playback(Arc::clone(revision), metered)?;
    let mut expected = vec![[0.0; 2]; VERIFY_FRAMES];
    let mut actual = vec![[0.0; 2]; VERIFY_FRAMES];
    for sample in expected.chunks_mut(1) {
        scalar.render(sample)?;
    }
    for chunk in actual.chunks_mut(BLOCK) {
        block.render(chunk)?;
    }
    if !actual
        .iter()
        .zip(&expected)
        .all(|(actual, expected)| actual.map(f32::to_bits) == expected.map(f32::to_bits))
        || !actual.iter().flatten().all(|v| v.is_finite())
        || !actual.iter().flatten().any(|v| *v != 0.0)
    {
        return Err("partitioned PCM differs, is non-finite, or is silent".into());
    }
    Ok(())
}

fn measure(label: &str, workload: Workload) -> Result<(), Box<dyn Error>> {
    let revision = prepared(workload)?;
    let metered = matches!(workload, Workload::MeteredNotes);
    verify(&revision, metered)?;
    let latency = revision.arrangement().main_latency_frames();
    let tracks = revision.arrangement().tracks().len();
    let mut playback = playback(revision, metered)?;
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..WARMUP {
        playback.render(&mut output)?;
    }
    let mut times = Vec::with_capacity(MEASURED);
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..MEASURED {
        let start = Instant::now();
        playback.render(black_box(&mut output))?;
        times.push(start.elapsed().as_nanos());
        for value in output.iter().flatten() {
            digest = (digest ^ u64::from(value.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
        }
        black_box(&output);
    }
    times.sort_unstable();
    println!(
        "case={label} tracks={tracks} rate={} block={BLOCK} latency_frames={latency} warmup={WARMUP} measured={MEASURED} p50_us={:.3} p99_us={:.3} max_us={:.3} pcm_digest={digest:016x}",
        session_audio_fixture::RATE,
        times[MEASURED / 2] as f64 / 1000.0,
        times[(MEASURED * 99).div_ceil(100) - 1] as f64 / 1000.0,
        times[MEASURED - 1] as f64 / 1000.0
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!(
        "compensated routing: arch={} os={} headless; source revision and host metadata recorded separately; setup and PCM oracle excluded; no native deadline claim",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    measure("track_latency", Workload::Track)?;
    measure("return_latency", Workload::Return)?;
    measure("mixed_latency", Workload::Mixed)?;
    measure("metered_notes", Workload::MeteredNotes)?;
    Ok(())
}
