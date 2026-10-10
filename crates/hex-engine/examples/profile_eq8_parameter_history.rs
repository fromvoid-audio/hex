//! Registered EQ8 compatible parameter-handoff profiler; headless, not a device deadline.
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, DeviceId, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
const RATE: u32 = 48_000;
const WARMUPS: usize = 200;
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let block: usize = args.first().map_or(Ok(128), |s| s.parse())?;
    let samples: usize = args.get(1).map_or(Ok(2_000), |s| s.parse())?;
    if args.len() > 2 || !matches!(block, 128 | 256) || !(100..=20_000).contains(&samples) {
        return Err("usage: profile_eq8_parameter_history [128|256] [100..20000]".into());
    }
    let (mut editor, device) = fixture()?;
    let table = Arc::new(SineTable::new());
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let mut playback = SessionPlayback::new(Arc::clone(&revision))?;
    playback.apply(SessionPlaybackCommand::Play)?;
    let mut output = [[0.0; 2]; 256];
    let mut handoff = Vec::with_capacity(samples);
    let mut render = Vec::with_capacity(samples);
    let mut checksum = 0_u64;
    let mut audible = false;
    for iteration in 0..WARMUPS + samples {
        let mut state = editor
            .project()
            .tracks()
            .first()
            .ok_or("missing track")?
            .devices()
            .iter()
            .find(|d| d.id() == device)
            .ok_or("missing eq8")?
            .state()
            .clone();
        for (id, value) in [
            (
                4,
                ParameterValue::Float(if iteration % 2 == 0 { 3.0 } else { 8.0 }),
            ),
            (
                5,
                ParameterValue::Float(if iteration % 3 == 0 { 2.0 } else { 1.2 }),
            ),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                &state,
                ParameterId::from_static(id),
                value,
            )?;
        }
        editor.execute(Command::ReplaceDeviceState { device, state })?;
        let prepared = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(revision.session()),
        )?);
        let mut candidate = SessionPlayback::new(Arc::clone(&prepared))?;
        let start = Instant::now();
        black_box(candidate.try_inherit_state_from(&mut playback)?);
        let handoff_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        candidate.render(black_box(&mut output[..block]))?;
        let render_ns = start.elapsed().as_nanos();
        if output[..block].iter().flatten().any(|s| !s.is_finite()) {
            return Err("nonfinite output".into());
        }
        if iteration >= WARMUPS {
            handoff.push(handoff_ns);
            render.push(render_ns);
            for s in output[..block].iter().flatten() {
                audible |= s.abs() > 1.0e-6;
                checksum = checksum
                    .wrapping_mul(31)
                    .wrapping_add(u64::from(s.to_bits()));
            }
        }
        // Owner destruction and preparation remain outside timed handoff/render.
        drop(std::mem::replace(&mut playback, candidate));
        drop(std::mem::replace(&mut revision, prepared));
    }
    if !audible {
        return Err("silent fixture".into());
    }
    report("handoff", block, samples, &mut handoff, checksum);
    report("following_render", block, samples, &mut render, checksum);
    Ok(())
}
fn report(phase: &str, block: usize, samples: usize, values: &mut [u128], checksum: u64) {
    values.sort_unstable();
    let p = |percent: usize| values[(values.len() - 1) * percent / 100] as f64 / 1_000.0;
    println!(
        "{{\"phase\":\"{phase}\",\"rate_hz\":{RATE},\"block\":{block},\"warmups\":{WARMUPS},\"samples\":{samples},\"p50_us\":{:.3},\"p95_us\":{:.3},\"p99_us\":{:.3},\"max_us\":{:.3},\"checksum\":{checksum}}}",
        p(50),
        p(95),
        p(99),
        p(100)
    );
}

fn fixture() -> Result<(Editor, DeviceId), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: "spectrum profile".into(),
    })?
    else {
        return Err("midi track creation returned another object".into());
    };
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )?;
    for (id, value) in [
        (1, ParameterValue::Bool(true)),
        (3, ParameterValue::Float(1_200.0)),
        (4, ParameterValue::Float(8.0)),
        (5, ParameterValue::Float(1.2)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Effect,
            &state,
            ParameterId::from_static(id),
            value,
        )?;
    }
    let Created::Device(eq8) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?
    else {
        return Err("eq8 insertion returned another object".into());
    };
    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(512)?,
    })?
    else {
        return Err("midi clip creation returned another object".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(512)?,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })?;
    Ok((editor, eq8))
}
