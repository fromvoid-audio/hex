//! Original prepared Operator hot-path measurement; no native deadline claim.
use hex_engine::{
    PreparedRevision,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

fn prepare(active: bool, sync: bool, feedback: bool) -> Result<Playback, Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(48_000)?, 128)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: "lfo profile".into(),
    })?
    else {
        return Err("track".into());
    };
    let device = editor
        .project()
        .track(track)
        .and_then(|t| t.instrument())
        .ok_or("instrument")?
        .id();
    let class = hex_engine::NativeDeviceClass::Instrument;
    let mut state = hex_engine::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;
    if feedback {
        state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            ParameterId::from_static(78),
            ParameterValue::Float(0.4),
        )?;
    }
    if active {
        for (id, value) in [
            (82, ParameterValue::Bool(true)),
            (84, ParameterValue::Bool(sync)),
            (85, ParameterValue::Float(7.0)),
            (87, ParameterValue::Float(75.0)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )?;
        }
    }
    editor.execute(Command::ReplaceDeviceState { device, state })?;
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(64)?,
    })?
    else {
        return Err("clip".into());
    };
    for note in 0..16 {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(64)?,
            pitch: 60 + note % 12,
            channel: 0,
            velocity: 100,
        })?;
    }
    let revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        hex_engine::SharedSineTable::new_on_control(),
        None,
    )?);
    let mut playback = Playback::try_from_prepared_with_session(revision)?;
    playback.apply(PlaybackCommand::Play);
    Ok(playback)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "neutral".into());
    let (active, sync, feedback) = match mode.as_str() {
        "neutral" => (false, false, false),
        "free" => (true, false, false),
        "sync" => (true, true, false),
        "feedback" => (false, false, true),
        _ => return Err("expected neutral, free, sync, or feedback".into()),
    };
    // Partition oracle runs before timing, with independently owned runtimes.
    let mut a = prepare(active, sync, feedback)?;
    let mut b = prepare(active, sync, feedback)?;
    let mut expected = vec![[0.0; 2]; 268_800];
    let mut actual = expected.clone();
    for block in expected.chunks_mut(128) {
        a.render(block)?;
    }
    for block in actual.chunks_mut(113) {
        b.render(block)?;
    }
    if actual != expected {
        return Err("partition PCM mismatch".into());
    }
    let hash = expected
        .iter()
        .flatten()
        .fold(0xcbf29ce484222325_u64, |h, v| {
            (h ^ u64::from(v.to_bits())).wrapping_mul(0x100000001b3)
        });
    println!("verified PCM hash={hash:016x}");
    let mut playback = prepare(active, sync, feedback)?;
    let mut output = [[0.0; 2]; 128];
    for _ in 0..100 {
        playback.render(&mut output)?;
        black_box(&output);
    }
    let mut durations = Vec::with_capacity(2000);
    let start = Instant::now();
    for _ in 0..2000 {
        let at = Instant::now();
        playback.render(&mut output)?;
        black_box(&output);
        durations.push(at.elapsed());
    }
    let total = start.elapsed();
    durations.sort_unstable();
    println!(
        "{mode}: 48khz,16 held voices,default routing,128 frames,100 warmups,2000 blocks,Instant/black_box; partition PCM exact"
    );
    println!(
        "total={:.6}s p50/p95/p99/max={:.3}/{:.3}/{:.3}/{:.3}us",
        total.as_secs_f64(),
        durations[1000].as_secs_f64() * 1e6,
        durations[1900].as_secs_f64() * 1e6,
        durations[1980].as_secs_f64() * 1e6,
        durations[1999].as_secs_f64() * 1e6
    );
    Ok(())
}
