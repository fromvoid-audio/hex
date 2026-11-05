//! Isolated control-stack probes for SessionPlayback construction and preroll.
//!
//! Run in debug and release with:
//! `cargo run -p hex-engine --features instrument-test-support --example playback_stack`
//! `cargo run --release -p hex-engine --features instrument-test-support --example playback_stack`
//!
//! Failed small-stack trials run in child processes: a Rust stack overflow aborts
//! its process, so it must never take down the invoking shell or test runner.

use hex_devices::{SINE_INSTRUMENT_TYPE_ID, SineTable, utility::UTILITY_DEFINITION};
use hex_engine::{PreparedRevision, SessionPlayback};
use hex_lattice::{NativeInstrumentDefinition, NativeInstrumentRegistry};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterValue};
use hex_project::{
    BeatTime, DeviceSnapshot, MidiClipSnapshot, MidiNoteSnapshot, Project, ReturnTrackSnapshot,
    SendLevelSnapshot, TrackContentSnapshot, TrackSnapshot,
};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    process::{Command, Output},
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::Instant,
};

const RATE: u32 = 48_000;
const MUTABLE_TYPE: DeviceTypeId = DeviceTypeId::from_static("hex.stack_probe.mutable");
const FLOOR_STACK: usize = 512 * 1024;
const PRODUCTION_STACK: usize = 8 * 1024 * 1024;
const STACK_RESOLUTION: usize = 1024 * 1024;
const TRACKS: usize = 100;
const NOTES_PER_TRACK: usize = 32;
const RELEASE_TARGET: u64 = 24_704;

#[path = "playback_stack/mutable_instrument.rs"]
mod mutable_instrument;
use mutable_instrument::{PROBE_INSTRUMENT, ProbeInstrumentDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fixture {
    Utility,
    DenseMutable,
}

impl Fixture {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "utility" => Ok(Self::Utility),
            "dense-mutable" => Ok(Self::DenseMutable),
            _ => Err(format!("unknown fixture: {value}").into()),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Utility => "utility",
            Self::DenseMutable => "dense-mutable",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--child")
    {
        let fixture = Fixture::parse(arguments.get(1).ok_or("missing child fixture")?)?;
        let stack = arguments
            .get(2)
            .ok_or("missing child stack size")?
            .parse::<usize>()?;
        return run_child(fixture, stack);
    }

    for fixture in [Fixture::Utility, Fixture::DenseMutable] {
        let result = probe_fixture(fixture)?;
        println!(
            "threshold fixture={} last_failing_stack={} first_passing_stack={} resolution={} production_8mib={}",
            fixture.name(),
            if result.lower_failed == 0 {
                "none (floor passed)".to_owned()
            } else {
                format_bytes(result.lower_failed)
            },
            format_bytes(result.upper_pass),
            format_bytes(STACK_RESOLUTION),
            if result.upper_pass <= PRODUCTION_STACK {
                "passed"
            } else {
                "passed with margin unquantified"
            }
        );
        println!(
            "pcm fixture={} held_digest={:016x} released_digest={:016x}",
            fixture.name(),
            result.held_digest,
            result.released_digest
        );
    }
    Ok(())
}

struct Threshold {
    lower_failed: usize,
    upper_pass: usize,
    held_digest: u64,
    released_digest: u64,
}

fn probe_fixture(fixture: Fixture) -> Result<Threshold, Box<dyn Error>> {
    let production =
        run_trial(fixture, PRODUCTION_STACK)?.ok_or("production 8 MiB probe stack overflowed")?;
    let floor = run_trial(fixture, FLOOR_STACK)?;
    let (lower_failed, upper_pass, expected) = if let Some(floor_digests) = floor {
        if floor_digests != production {
            return Err("PCM changed between 512 KiB and 8 MiB stack runs".into());
        }
        (0, FLOOR_STACK, floor_digests)
    } else {
        let mut lower_failed = FLOOR_STACK;
        let mut upper_pass = PRODUCTION_STACK;
        while upper_pass - lower_failed > STACK_RESOLUTION {
            let midpoint = ((lower_failed + upper_pass) / (2 * STACK_RESOLUTION))
                .checked_mul(STACK_RESOLUTION)
                .ok_or("stack probe midpoint overflow")?;
            let candidate = midpoint.max(FLOOR_STACK);
            if candidate <= lower_failed || candidate >= upper_pass {
                break;
            }
            match run_trial(fixture, candidate)? {
                Some(digests) => {
                    if digests != production {
                        return Err(format!("PCM changed at {} stack bytes", candidate).into());
                    }
                    upper_pass = candidate;
                }
                None => lower_failed = candidate,
            }
        }
        (lower_failed, upper_pass, production)
    };

    Ok(Threshold {
        lower_failed,
        upper_pass,
        held_digest: expected.0,
        released_digest: expected.1,
    })
}

fn run_trial(fixture: Fixture, stack_bytes: usize) -> Result<Option<(u64, u64)>, Box<dyn Error>> {
    let executable = env::current_exe()?;
    let started = Instant::now();
    let output = Command::new(executable)
        .arg("--child")
        .arg(fixture.name())
        .arg(stack_bytes.to_string())
        .output()?;
    let elapsed = started.elapsed();
    if output.status.success() {
        let digests = parse_child_digests(&output)?;
        println!(
            "trial fixture={} stack={} result=passed elapsed_ms={:.3}",
            fixture.name(),
            format_bytes(stack_bytes),
            elapsed.as_secs_f64() * 1_000.0
        );
        return Ok(Some(digests));
    }

    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if diagnostics.to_ascii_lowercase().contains("stack overflow") {
        println!(
            "trial fixture={} stack={} result=stack-overflow elapsed_ms={:.3}",
            fixture.name(),
            format_bytes(stack_bytes),
            elapsed.as_secs_f64() * 1_000.0
        );
        return Ok(None);
    }
    Err(format!(
        "isolated probe failed without a stack-overflow diagnostic (status={}): {}",
        output.status,
        diagnostics.trim()
    )
    .into())
}

fn parse_child_digests(output: &Output) -> Result<(u64, u64), Box<dyn Error>> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    let line = stdout
        .lines()
        .find(|line| line.starts_with("digests "))
        .ok_or("child did not emit its PCM digests")?;
    let mut fields = line.split_whitespace();
    let _ = fields.next();
    let held = fields
        .next()
        .and_then(|field| field.strip_prefix("held="))
        .ok_or("child held digest is missing")?
        .parse::<u64>()?;
    let released = fields
        .next()
        .and_then(|field| field.strip_prefix("released="))
        .ok_or("child released digest is missing")?
        .parse::<u64>()?;
    Ok((held, released))
}

fn run_child(fixture: Fixture, stack_bytes: usize) -> Result<(), Box<dyn Error>> {
    if !(FLOOR_STACK..=PRODUCTION_STACK).contains(&stack_bytes) {
        return Err("child stack must be within the 512 KiB..8 MiB probe range".into());
    }
    let revision = prepare_fixture(fixture)?;
    let started = Instant::now();
    let worker = thread::Builder::new()
        .name("hex-playback-stack-worker".to_owned())
        .stack_size(stack_bytes)
        .spawn(move || -> Result<(u64, u64), String> {
            let held = render_preroll(Arc::clone(&revision), 512)?;
            let released = render_preroll(revision, RELEASE_TARGET)?;
            Ok((held, released))
        })?;
    let (held, released) = worker
        .join()
        .map_err(|_| "stack probe worker panicked")?
        .map_err(|error| -> Box<dyn Error> { error.into() })?;
    println!(
        "digests held={held} released={released} elapsed_ms={:.3}",
        started.elapsed().as_secs_f64() * 1_000.0
    );
    Ok(())
}

fn render_preroll(revision: Arc<PreparedRevision>, target: u64) -> Result<u64, String> {
    let mut playback = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
        revision,
        target,
        target,
        true,
        &AtomicBool::new(false),
    )
    .map_err(|error| format!("interactive preroll refused: {error:?}"))?;
    if playback.performance_frame() != target || playback.arrangement_frame() != target {
        return Err("prepared playback clock does not match its target".to_owned());
    }
    let mut output = [[0.0_f32; 2]; 256];
    playback
        .render(&mut output)
        .map_err(|error| error.to_string())?;
    if output.iter().flatten().any(|sample| !sample.is_finite())
        || !output.iter().flatten().any(|sample| *sample != 0.0)
    {
        return Err("fixture PCM is silent or non-finite".to_owned());
    }
    Ok(output
        .iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3)
        }))
}

fn prepare_fixture(fixture: Fixture) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let (project, definition) = project_fixture(fixture)?;
    let sine_table = Arc::new(SineTable::new());
    let revision = match definition {
        Some(definition) => {
            let definitions: [&dyn NativeInstrumentDefinition; 1] = [definition];
            let registry = NativeInstrumentRegistry::new(&definitions)?;
            PreparedRevision::prepare_with_test_instrument_registry(
                &project,
                &BTreeMap::new(),
                sine_table,
                None,
                &registry,
            )?
        }
        None => PreparedRevision::prepare(&project, &BTreeMap::new(), sine_table, None)?,
    };
    Ok(Arc::new(revision))
}

fn project_fixture(
    fixture: Fixture,
) -> Result<(Project, Option<&'static ProbeInstrumentDefinition>), Box<dyn Error>> {
    let track_count = if fixture == Fixture::DenseMutable {
        TRACKS
    } else {
        1
    };
    let instrument_type = if fixture == Fixture::DenseMutable {
        MUTABLE_TYPE
    } else {
        SINE_INSTRUMENT_TYPE_ID
    };
    let instrument_definition = (fixture == Fixture::DenseMutable).then_some(&PROBE_INSTRUMENT);
    let mut utility_state = UTILITY_DEFINITION.default_state()?;
    utility_state = UTILITY_DEFINITION.patch_parameter(
        &utility_state,
        hex_devices::utility::parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    )?;
    let mut next_id = 1_u64;
    let mut tracks = Vec::with_capacity(track_count);
    for track_index in 0..track_count {
        let track_id = next_id;
        next_id += 1;
        let instrument_id = next_id;
        next_id += 1;
        let effect_id = next_id;
        next_id += 1;
        let clip_id = next_id;
        next_id += 1;
        let mut notes = Vec::new();
        if fixture == Fixture::DenseMutable {
            notes.reserve(NOTES_PER_TRACK);
            for note_index in 0..NOTES_PER_TRACK {
                notes.push(MidiNoteSnapshot {
                    active: true,
                    id: next_id,
                    start_raw: 0,
                    duration_raw: BeatTime::ONE.raw(),
                    pitch: 36 + (note_index % 48) as u8,
                    channel: 0,
                    velocity: 100,
                });
                next_id += 1;
            }
        } else {
            notes.push(MidiNoteSnapshot {
                active: true,
                id: next_id,
                start_raw: 0,
                duration_raw: BeatTime::from_quarters(2)?.raw(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            });
            next_id += 1;
        }
        tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("stack probe {track_index:03}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: vec![DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: effect_id,
                bypassed: false,
                state: utility_state.clone(),
            }],
            instrument: Some(DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: NativeDeviceState::new(instrument_type, 1, Vec::new())?,
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Hybrid {
                audio_clips: Vec::new(),
                midi_clips: vec![MidiClipSnapshot {
                    content_view: None,
                    active: true,
                    playback_offset_raw: 0,
                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                    volume: hex_project::MidiClipVolume::default(),
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    id: clip_id,
                    start_raw: 0,
                    length_raw: BeatTime::from_quarters(2)?.raw(),
                    source_length_raw: BeatTime::from_quarters(2)?.raw(),
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: BeatTime::from_quarters(2)?.raw(),
                    looping: false,
                    notes,
                }],
            },
            sends: vec![SendLevelSnapshot {
                return_track: 0,
                gain: 0.5,
            }],
        });
    }
    let return_id = next_id;
    for track in &mut tracks {
        track.sends[0].return_track = return_id;
    }
    next_id += 1;
    let return_effect_id = next_id;
    next_id += 1;
    let mut snapshot = Project::new(RATE)?.snapshot();
    snapshot.next_id = next_id;
    snapshot.tracks = tracks;
    snapshot.return_tracks = vec![ReturnTrackSnapshot {
        id: return_id,
        name: "stack probe return".to_owned(),
        gain: 1.0,
        muted: false,
        devices: vec![DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: return_effect_id,
            bypassed: false,
            state: utility_state,
        }],
    }];
    Ok((Project::from_snapshot(snapshot)?, instrument_definition))
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 && bytes.is_multiple_of(1024 * 1024) {
        format!("{} MiB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}
