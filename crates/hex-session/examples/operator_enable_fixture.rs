//! Create a saved Operator arrangement for listening to held-note enable edits.
//!
//! Run with `cargo run -p hex-session --example operator_enable_fixture -- analytic ./operator-analytic.hex`.
//! The MIDI arrangement holds pitch 48 for 30 seconds, then leaves the three-second
//! Operator release and additional silence inside the clip.
use hex_engine::{NativeDeviceClass, PreparedRevision, SineTable};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, path::Path, sync::Arc};

const SAMPLE_RATE: u32 = 48_000;
const NOTE_START_BEATS: u64 = 4;
const NOTE_DURATION_BEATS: u64 = 60;
const CLIP_LENGTH_BEATS: u64 = 74;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Analytic,
    Serial,
    Feedback,
    FreeLfo,
    SyncedLfo,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "analytic" => Ok(Self::Analytic),
            "serial" => Ok(Self::Serial),
            "feedback" => Ok(Self::Feedback),
            "free" => Ok(Self::FreeLfo),
            "sync" => Ok(Self::SyncedLfo),
            _ => Err("expected analytic, serial, feedback, free, or sync"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Analytic => "analytic",
            Self::Serial => "serial",
            Self::Feedback => "feedback",
            Self::FreeLfo => "free",
            Self::SyncedLfo => "sync",
        }
    }
}

fn patch(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: u32,
    value: ParameterValue,
) -> Result<hex_lattice_contract::NativeDeviceState, Box<dyn Error>> {
    Ok(hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        state,
        ParameterId::from_static(parameter),
        value,
    )?)
}

fn project(mode: Mode) -> Result<Project, Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(SAMPLE_RATE)?, 16)?;
    let Created::Track(track) = editor.execute(Command::AddMidiTrack {
        name: format!("operator {} held note", mode.name()),
    })?
    else {
        return Err("midi track creation returned no track id".into());
    };
    let mut state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )?;

    // Start from the authored parallel-carrier diagnostic patch. Every oscillator
    // has a finite sustain/release envelope and all four enable switches stay on.
    state = patch(&state, 61, ParameterValue::Choice(2))?;
    for (id, value) in [
        (2, 1.0),
        (5, 1.0),
        (8, 1.0),
        (11, 1.0),
        (29, 0.25),
        (30, 0.25),
        (31, 0.25),
        (32, 0.25),
    ] {
        state = patch(&state, id, ParameterValue::Float(value))?;
    }
    for operator in 0..4 {
        let envelope = 33 + operator * 6;
        for (offset, value) in [(0, 0.0), (1, 1.0), (2, 0.7), (3, 0.005), (4, 0.1), (5, 3.0)] {
            state = patch(&state, envelope + offset, ParameterValue::Float(value))?;
        }
        state = patch(&state, 513 + operator, ParameterValue::Bool(true))?;
    }

    match mode {
        Mode::Analytic => {}
        Mode::Serial => state = patch(&state, 61, ParameterValue::Choice(1))?,
        Mode::Feedback => {
            for parameter in 78..=81 {
                state = patch(&state, parameter, ParameterValue::Float(0.3))?;
            }
        }
        Mode::FreeLfo | Mode::SyncedLfo => {
            state = patch(&state, 82, ParameterValue::Bool(true))?;
            state = patch(&state, 84, ParameterValue::Bool(mode == Mode::SyncedLfo))?;
            state = patch(&state, 85, ParameterValue::Float(1.0))?;
            state = patch(&state, 86, ParameterValue::Choice(4))?;
            state = patch(&state, 87, ParameterValue::Float(35.0))?;
        }
    }
    editor.execute(Command::InsertInstrument { track, state })?;
    // Keep the output conservative for a diagnostic patch with four carriers.
    editor.execute(Command::MasterGain { gain: 0.5 })?;

    let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(CLIP_LENGTH_BEATS)?,
    })?
    else {
        return Err("midi clip creation returned no clip id".into());
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::from_quarters(NOTE_START_BEATS)?,
        duration: BeatTime::from_quarters(NOTE_DURATION_BEATS)?,
        pitch: 48,
        channel: 0,
        velocity: 100,
    })?;
    Ok(editor.project().clone())
}

fn save(mode: Mode, destination: &Path) -> Result<Project, Box<dyn Error>> {
    let project = project(mode)?;
    // Check the authored project through the same built-in registry and
    // Arrangement compiler used by playback before publishing the file.
    let _prepared =
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)?;
    hex_storage::save_new(destination, &project)?;
    println!(
        "saved {} operator held-note fixture to {} (48 khz, pitch 48, 30 s gate, 3 s release)",
        mode.name(),
        destination.display()
    );
    Ok(project)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .ok_or("expected mode and output path")?
        .into_string()
        .map_err(|_| "mode must be valid utf-8")?;
    let mode = Mode::parse(&mode)?;
    let destination = args.next().ok_or("expected mode and output path")?;
    if args.next().is_some() {
        return Err("expected exactly a mode and output path".into());
    }
    save(mode, Path::new(&destination)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::playback::{Playback, PlaybackCommand};
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    const MODES: [Mode; 5] = [
        Mode::Analytic,
        Mode::Serial,
        Mode::Feedback,
        Mode::FreeLfo,
        Mode::SyncedLfo,
    ];
    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(mode: Mode) -> Result<std::path::PathBuf, Box<dyn Error>> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let serial = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        Ok(std::env::temp_dir().join(format!(
            "hex-operator-enable-{}-{timestamp}-{serial}-{}.hex",
            std::process::id(),
            mode.name()
        )))
    }

    fn rendered_windows(playback: &mut Playback) -> Result<(f64, f64, f64), Box<dyn Error>> {
        const BLOCK: usize = 128;
        const TOTAL_FRAMES: usize = 37 * SAMPLE_RATE as usize;
        const GATE_START: usize = 28 * SAMPLE_RATE as usize;
        const GATE_END: usize = 29 * SAMPLE_RATE as usize;
        const RELEASE_START: usize = 32 * SAMPLE_RATE as usize + SAMPLE_RATE as usize / 2;
        const RELEASE_END: usize = 33 * SAMPLE_RATE as usize + SAMPLE_RATE as usize / 2;
        const SILENCE_START: usize = 36 * SAMPLE_RATE as usize;
        let mut output = [[0.0_f32; 2]; BLOCK];
        let mut gate = (0.0_f64, 0_u64);
        let mut release = (0.0_f64, 0_u64);
        let mut silence = (0.0_f64, 0_u64);
        let mut frame = 0;
        while frame < TOTAL_FRAMES {
            let count = BLOCK.min(TOTAL_FRAMES - frame);
            playback.render(&mut output[..count])?;
            for (offset, sample) in output[..count].iter().enumerate() {
                if sample.iter().any(|channel| !channel.is_finite()) {
                    return Err("Operator fixture rendered a non-finite sample".into());
                }
                let at = frame + offset;
                let window = if (GATE_START..GATE_END).contains(&at) {
                    Some(&mut gate)
                } else if (RELEASE_START..RELEASE_END).contains(&at) {
                    Some(&mut release)
                } else if at >= SILENCE_START {
                    Some(&mut silence)
                } else {
                    None
                };
                if let Some((sum, count)) = window {
                    for channel in sample {
                        *sum += f64::from(*channel).powi(2);
                        *count += 1;
                    }
                }
            }
            frame += count;
        }
        let rms = |(sum, count): (f64, u64)| (sum / count.max(1) as f64).sqrt();
        Ok((rms(gate), rms(release), rms(silence)))
    }

    #[test]
    fn saved_fixtures_roundtrip_note_boundaries_and_render_gate_release_and_silence()
    -> Result<(), Box<dyn Error>> {
        for mode in MODES {
            let path = temporary_path(mode)?;
            let result = (|| -> Result<(), Box<dyn Error>> {
                let authored = save(mode, &path)?;
                let saved_bytes = fs::read(&path)?;
                assert!(
                    save(mode, &path).is_err(),
                    "existing fixture must not be overwritten"
                );
                assert_eq!(fs::read(&path)?, saved_bytes);
                let loaded = hex_storage::load(&path)?;
                assert_eq!(
                    loaded, authored,
                    "saved settings and project state must roundtrip"
                );
                let track = loaded
                    .tracks()
                    .first()
                    .ok_or("loaded fixture has no track")?;
                let clip = track
                    .midi_clips()
                    .first()
                    .ok_or("loaded fixture has no midi clip")?;
                let note = clip.notes().first().ok_or("loaded fixture has no note")?;
                assert_eq!(clip.start(), BeatTime::ZERO);
                assert_eq!(clip.length(), BeatTime::from_quarters(CLIP_LENGTH_BEATS)?);
                assert_eq!(note.start(), BeatTime::from_quarters(NOTE_START_BEATS)?);
                assert_eq!(
                    note.duration(),
                    BeatTime::from_quarters(NOTE_DURATION_BEATS)?
                );
                assert_eq!(note.pitch(), 48);

                let revision = Arc::new(PreparedRevision::prepare(
                    &loaded,
                    &BTreeMap::new(),
                    Arc::new(SineTable::new()),
                    None,
                )?);
                let mut playback = Playback::try_from_prepared_with_session(revision)?;
                playback.apply(PlaybackCommand::Play);
                let (gate, release, silence) = rendered_windows(&mut playback)?;
                assert!(
                    gate.is_finite() && gate > 1.0e-4,
                    "{mode:?} gate rms {gate}"
                );
                assert!(
                    release.is_finite() && release > 1.0e-6,
                    "{mode:?} release rms {release}"
                );
                assert!(
                    silence.is_finite() && silence < 1.0e-6,
                    "{mode:?} silence rms {silence}"
                );
                Ok(())
            })();
            let _ = fs::remove_file(&path);
            result?;
        }
        Ok(())
    }
}
