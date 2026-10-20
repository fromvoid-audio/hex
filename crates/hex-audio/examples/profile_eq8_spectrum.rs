//! Headless EQ8 output-tap timing comparison; no CPAL device or UI.
use hex_audio::{
    prepared,
    spectrum_worker::{CaptureOutcome, SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker},
};
use hex_engine::{PreparedRevision, playback::Playback};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, DeviceId, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const WARMUPS: usize = 200;
const DEFAULT_SAMPLES: usize = 4_000;
const MAX_SAMPLES: usize = 20_000;

#[derive(Clone, Copy)]
enum Mode {
    NoWorker,
    WorkerDisabled,
    Enabled,
}

impl Mode {
    const fn name(self) -> &'static str {
        match self {
            Self::NoWorker => "no_worker",
            Self::WorkerDisabled => "worker_target_off",
            Self::Enabled => "eq8_spectrum_enabled",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let (block, samples) = match arguments.as_slice() {
        [] => (128, DEFAULT_SAMPLES),
        [block] => (block.parse::<usize>()?, DEFAULT_SAMPLES),
        [block, samples] => (block.parse::<usize>()?, samples.parse::<usize>()?),
        _ => return Err("usage: profile_eq8_spectrum [128|256] [sample count]".into()),
    };
    if !matches!(block, 128 | 256) || !(100..=MAX_SAMPLES).contains(&samples) {
        return Err(
            format!("block must be 128 or 256; samples must be 100..={MAX_SAMPLES}").into(),
        );
    }

    let (revision, track_id, eq8) = fixture()?;
    for mode in [Mode::NoWorker, Mode::WorkerDisabled, Mode::Enabled] {
        run(mode, Arc::clone(&revision), track_id, eq8, block, samples)?;
    }
    Ok(())
}

fn fixture() -> Result<(Arc<PreparedRevision>, u64, DeviceId), Box<dyn Error>> {
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
    Ok((
        Arc::new(PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            hex_engine::SharedSineTable::new_on_control(),
            None,
        )?),
        track.value(),
        eq8,
    ))
}

fn run(
    mode: Mode,
    revision: Arc<PreparedRevision>,
    track_id: u64,
    device: DeviceId,
    block: usize,
    samples: usize,
) -> Result<(), Box<dyn Error>> {
    let initial_stamp = SpectrumStamp {
        target: SpectrumTargetId(0),
        generation: 0,
        sample_rate_hz: RATE,
        applied_revision: 0,
    };
    let (mut control, mut callback, mut reader, mut reaper) = match mode {
        Mode::NoWorker => {
            let (control, callback) = prepared::channel(RATE)?;
            (control, callback, None, None)
        }
        Mode::WorkerDisabled | Mode::Enabled => {
            let (capture, reader, reaper) = spawn_spectrum_worker(initial_stamp)?;
            let (control, callback) = prepared::channel_with_spectrum(RATE, Some(capture))?;
            (control, callback, Some(reader), Some(reaper))
        }
    };
    let publication = control
        .publish_with_policy_and_context_generation(
            Playback::try_from_prepared_with_session(Arc::clone(&revision))?,
            prepared::PublicationPolicy::InheritRuntime,
            1,
        )
        .map_err(|error| format!("{error:?}"))?;
    control
        .command(hex_engine::PlaybackCommand::Play)
        .map_err(|error| format!("{error:?}"))?;
    let mut oracle = Playback::try_from_prepared_with_session(revision)?;
    oracle.apply(hex_engine::PlaybackCommand::Play);
    if matches!(mode, Mode::Enabled) {
        control
            .spectrum_target
            .select(
                hex_engine::PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Track(track_id),
                    device.value(),
                    DeviceTypeId::from_static("hex.eq8"),
                ),
                1,
                publication,
                RATE,
            )
            .map_err(|error| format!("{error:?}"))?;
    }

    let mut elapsed = Vec::with_capacity(samples);
    let mut output = [[0.0; 2]; 256];
    let mut expected = output;
    let mut checksum = 0_u64;
    let mut non_silent = false;
    let mut queued = 0_usize;
    let mut queue_full = 0_usize;
    callback.begin_block();
    for sample in 0..WARMUPS + samples {
        let start = Instant::now();
        callback.begin_block();
        callback.render(&mut output[..block])?;
        let capture = callback.finish_spectrum_block(start, true);
        let nanos = start.elapsed().as_nanos();
        if sample >= WARMUPS {
            elapsed.push(nanos);
        }

        match (mode, capture) {
            (Mode::Enabled, Some(CaptureOutcome::Queued { .. })) => queued += 1,
            (Mode::Enabled, Some(CaptureOutcome::QueueFull { .. })) => queue_full += 1,
            (Mode::NoWorker | Mode::WorkerDisabled, None) => {}
            _ => {
                return Err(
                    format!("{} unexpected capture outcome: {capture:?}", mode.name()).into(),
                );
            }
        }
        oracle.render(&mut expected[..block])?;
        if output[..block] != expected[..block] {
            return Err(format!(
                "{}/{} callback PCM differs from oracle",
                mode.name(),
                sample
            )
            .into());
        }
        for value in output[..block].iter().flatten() {
            non_silent |= value.abs() > 1.0e-6;
            checksum = checksum
                .wrapping_mul(31)
                .wrapping_add(u64::from(value.to_bits()));
        }
        black_box(&output[..block]);
    }
    if !non_silent {
        return Err(format!("{} produced only silence", mode.name()).into());
    }
    if matches!(mode, Mode::Enabled) && queued == 0 {
        return Err("enabled analyzer never queued a real post-node capture".into());
    }
    if let Some(reader) = reader.as_mut() {
        let _ = reader.try_latest();
    }
    report(
        mode,
        block,
        samples,
        &mut elapsed,
        checksum,
        [queued, queue_full],
    );

    control.request_shutdown();
    callback.begin_block();
    callback.render(&mut output[..block])?;
    drop(callback);
    control.reclaim();
    if let Some(reaper) = reaper.take() {
        reaper.shutdown_and_join()?;
    }
    Ok(())
}

fn report(
    mode: Mode,
    block: usize,
    samples: usize,
    values: &mut [u128],
    checksum: u64,
    captures: [usize; 2],
) {
    values.sort_unstable();
    let percentile = |percent: usize| -> f64 {
        let index = (values.len() - 1) * percent / 100;
        values[index] as f64 / 1_000.0
    };
    println!(
        "{{\"scenario\":\"{}\",\"rate_hz\":{RATE},\"block\":{block},\"warmups\":{WARMUPS},\"samples\":{samples},\"p50_us\":{:.3},\"p95_us\":{:.3},\"max_us\":{:.3},\"checksum\":{checksum},\"queued_captures\":{},\"queue_full_captures\":{}}}",
        mode.name(),
        percentile(50),
        percentile(95),
        values[values.len() - 1] as f64 / 1_000.0,
        captures[0],
        captures[1],
    );
}
