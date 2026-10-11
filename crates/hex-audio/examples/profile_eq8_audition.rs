//! Headless EQ8 audition lease timing and PCM oracles; no CPAL device or UI.
use hex_audio::prepared;
use hex_engine::{PreparedRevision, ProcessorAuditionSelection, SineTable, playback::Playback};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, DeviceId, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const WARMUPS: usize = 200;
const DEFAULT_SAMPLES: usize = 4_000;
const MAX_SAMPLES: usize = 20_000;

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Held,
    Released,
    Cancelled,
}
impl Mode {
    const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Held => "held_audition",
            Self::Released => "released_audition",
            Self::Cancelled => "cancelled_audition",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let (block, samples) = match arguments.as_slice() {
        [] => (128, DEFAULT_SAMPLES),
        [block] => (block.parse::<usize>()?, DEFAULT_SAMPLES),
        [block, samples] => (block.parse::<usize>()?, samples.parse::<usize>()?),
        _ => return Err("usage: profile_eq8_audition [128|256] [sample count]".into()),
    };
    if !matches!(block, 128 | 256) || !(100..=MAX_SAMPLES).contains(&samples) {
        return Err(
            format!("block must be 128 or 256; samples must be 100..={MAX_SAMPLES}").into(),
        );
    }

    let (revision, track_id, eq8) = fixture()?;
    for mode in [Mode::Normal, Mode::Held, Mode::Released, Mode::Cancelled] {
        run(mode, Arc::clone(&revision), track_id, eq8, block, samples)?;
    }
    Ok(())
}

fn fixture() -> Result<(Arc<PreparedRevision>, u64, DeviceId), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: "audition profile".into(),
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
        (6, ParameterValue::Bool(true)),
        (7, ParameterValue::Choice(1)),
        (8, ParameterValue::Float(400.0)),
        (9, ParameterValue::Float(-9.0)),
        (10, ParameterValue::Float(0.8)),
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
            Arc::new(SineTable::new()),
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
    let (mut control, mut callback) = prepared::channel(RATE)?;
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
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let mut normal_oracle = Playback::try_from_prepared_with_session(revision)?;
    oracle.apply(hex_engine::PlaybackCommand::Play);
    normal_oracle.apply(hex_engine::PlaybackCommand::Play);
    let identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track_id),
        device.value(),
        DeviceTypeId::from_static("hex.eq8"),
    );
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    if !matches!(mode, Mode::Normal) {
        control
            .processor_audition
            .select(identity, 1, publication, RATE, selection)
            .map_err(|error| format!("{error:?}"))?;
        if !oracle.select_processor_audition(identity, Some(selection)) {
            return Err("separate playback oracle could not activate audition".into());
        }
    }

    let mut elapsed = Vec::with_capacity(samples);
    let mut output = [[0.0; 2]; 256];
    let mut expected = output;
    let mut normal_expected = output;
    let mut checksum = 0_u64;
    let mut non_silent = false;
    let mut differs_from_normal = false;
    callback.begin_block();
    for sample in 0..WARMUPS + samples {
        if sample == WARMUPS / 2 {
            match mode {
                Mode::Released => {
                    control
                        .processor_audition
                        .release()
                        .map_err(|error| format!("{error:?}"))?;
                    if !oracle.select_processor_audition(identity, None) {
                        return Err("separate playback oracle could not release audition".into());
                    }
                }
                Mode::Cancelled => {
                    control
                        .processor_audition
                        .cancel_immediately()
                        .map_err(|error| format!("{error:?}"))?;
                    oracle.cancel_all_processor_auditions();
                }
                Mode::Normal | Mode::Held => {}
            }
        }
        let start = Instant::now();
        callback.begin_block();
        callback.render(&mut output[..block])?;
        let capture = callback.finish_spectrum_block(start, true);
        let nanos = start.elapsed().as_nanos();
        if sample >= WARMUPS {
            elapsed.push(nanos);
        }

        if capture.is_some() {
            return Err("unexpected spectrum capture".into());
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
        normal_oracle.render(&mut normal_expected[..block])?;
        if sample >= WARMUPS {
            let differs = output[..block] != normal_expected[..block];
            differs_from_normal |= differs;
            if !matches!(mode, Mode::Held) && differs {
                return Err(format!("{} did not return exactly to normal PCM", mode.name()).into());
            }
        }
        for value in output[..block].iter().flatten() {
            non_silent |= value.abs() > 1.0e-6;
            if sample >= WARMUPS {
                checksum = checksum
                    .wrapping_mul(31)
                    .wrapping_add(u64::from(value.to_bits()));
            }
        }
        black_box(&output[..block]);
    }
    if !non_silent {
        return Err(format!("{} produced only silence", mode.name()).into());
    }
    if matches!(mode, Mode::Held) && !differs_from_normal {
        return Err("held audition never changed actual callback PCM".into());
    }
    report(mode, block, samples, &mut elapsed, checksum);

    control.request_shutdown();
    callback.begin_block();
    callback.render(&mut output[..block])?;
    drop(callback);
    control.reclaim();
    Ok(())
}

fn report(mode: Mode, block: usize, samples: usize, values: &mut [u128], checksum: u64) {
    values.sort_unstable();
    let percentile = |percent: usize| -> f64 {
        let index = (values.len() - 1) * percent / 100;
        values[index] as f64 / 1_000.0
    };
    println!(
        "{{\"scenario\":\"{}\",\"rate_hz\":{RATE},\"block\":{block},\"warmups\":{WARMUPS},\"samples\":{samples},\"p50_us\":{:.3},\"p95_us\":{:.3},\"max_us\":{:.3},\"checksum\":{checksum}}}",
        mode.name(),
        percentile(50),
        percentile(95),
        values[values.len() - 1] as f64 / 1_000.0,
    );
}
