//! Optimized, headless component timing for one Auto Pan per track. This excludes
//! native APIs, streaming, worker publication and physical output deadlines.
use hex_devices::auto_pan::{AUTO_PAN_DEFINITION, AUTO_PAN_TYPE_ID, parameter};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created, Editor,
    Project,
};
use std::{collections::BTreeMap, error::Error, sync::Arc, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let tracks: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "100".into())
        .parse()?;
    if !(1..=128).contains(&tracks) {
        return Err("track count must be within1..128".into());
    }
    const RATE: u32 = 48_000;
    const BLOCK: usize = 256;
    const WARMUP: usize = 32;
    const BLOCKS: usize = 256;
    let frames = (WARMUP + BLOCKS) * BLOCK;
    let audio = Audio::new(RATE, vec![[0.125, -0.25]; frames])?;
    // This SAME probe supports the pre-adoption source: static-only invokes no
    // new constructor or automation consumer and supplies a real static baseline.
    let static_only = std::env::args().any(|arg| arg == "--static-only");
    let modes: &[&str] = if static_only {
        &["static"]
    } else {
        &["static", "held_equivalent", "amount_curve", "all_curves"]
    };
    for &mode in modes {
        let automated = mode != "static";
        let mut editor = Editor::new(Project::new(RATE)?, 4)?;
        let mut media = BTreeMap::new();
        for index in 0..tracks {
            let Created::Imported { track, asset, .. } =
                editor.execute(Command::ImportPreparedAudioTrack {
                    name: format!("original {index}"),
                    path: format!("original-{index}.wav").into(),
                    source_sample_rate: RATE,
                    source_frames: frames as u64,
                    playback_frames: frames as u64,
                    conversion_version: hex_project::IDENTITY_CONVERSION,
                })?
            else {
                return Err("unexpected import result".into());
            };
            media.insert(asset, audio.clone());
            let Created::Device(device) = editor.execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state(AUTO_PAN_TYPE_ID)?,
            })?
            else {
                return Err("unexpected device result".into());
            };
            if mode == "all_curves" || mode == "held_equivalent" {
                let state = AUTO_PAN_DEFINITION.default_state()?;
                for descriptor in AUTO_PAN_DEFINITION.descriptors() {
                    if descriptor.id == parameter::AMOUNT && mode != "held_equivalent" {
                        continue;
                    }
                    let semantics = AUTO_PAN_DEFINITION
                        .parameter_semantics_for_state(&state, descriptor.id)?
                        .ok_or("undeclared auto pan parameter")?;
                    let normalized = semantics.normalization.normalize(
                        *descriptor,
                        AUTO_PAN_DEFINITION.read_parameter(&state, descriptor.id)?,
                    )?;
                    editor.execute(Command::ReplaceDeviceAutomation {
                        device,
                        parameter: descriptor.id,
                        points: vec![AutomationPoint {
                            beat: BeatTime::ZERO,
                            value: AutomationValue::new(normalized)?,
                            outgoing: AutomationInterpolation::Step,
                        }],
                    })?;
                }
            }
            if automated && mode != "held_equivalent" {
                editor.execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter: parameter::AMOUNT,
                    points: vec![
                        AutomationPoint {
                            beat: BeatTime::ZERO,
                            value: AutomationValue::new(0.25)?,
                            outgoing: AutomationInterpolation::Linear,
                        },
                        AutomationPoint {
                            beat: BeatTime::from_quarters(4)?,
                            value: AutomationValue::new(0.75)?,
                            outgoing: AutomationInterpolation::Step,
                        },
                    ],
                })?;
            }
        }
        let revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
        )?);
        // Control-side layout accounting, outside warmup and measured scopes.
        // Exact flat backing and concrete twin payloads; this deliberately does
        // not claim complete graph/asset/allocator resident memory.
        let lanes = if mode == "all_curves" || mode == "held_equivalent" {
            4
        } else {
            usize::from(automated)
        };
        let points = if mode == "all_curves" {
            5
        } else if mode == "held_equivalent" {
            4
        } else if automated {
            2
        } else {
            0
        };
        let program_bytes =
            hex_lattice::PreparedParameterAutomation::quoted_storage_bytes(lanes, points)
                .ok_or("program storage overflow")?;
        let state = AUTO_PAN_DEFINITION.default_state()?;
        let factory = AUTO_PAN_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: RATE,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )?
            .ok_or("missing auto pan factory")?;
        let pair_bytes = factory
            .declared_runtime_bytes()
            .checked_mul(2)
            .ok_or("runtime storage overflow")?;
        println!(
            "layout mode={mode} lanes_per_device={lanes} points_per_device={points} program_backing_per_device={program_bytes} concrete_runtime_pair_per_device={pair_bytes}"
        );
        let mut playback = SessionPlayback::new(revision)?;
        playback.apply(SessionPlaybackCommand::Play)?;
        let mut block = [[0.0; 2]; BLOCK];
        for _ in 0..WARMUP {
            playback.render(&mut block)?;
        }
        let mut times = Vec::with_capacity(BLOCKS);
        let started = Instant::now();
        for _ in 0..BLOCKS {
            let start = Instant::now();
            playback.render(&mut block)?;
            times.push(start.elapsed().as_nanos());
        }
        let elapsed = started.elapsed().as_secs_f64();
        times.sort_unstable();
        let p99 = times
            .get((BLOCKS * 99) / 100)
            .copied()
            .ok_or("timing sample unavailable")?;
        let maximum = times.last().copied().ok_or("timing sample unavailable")?;
        let rendered_seconds = BLOCKS as f64 * BLOCK as f64 / f64::from(RATE);
        println!(
            "mode={} tracks={tracks} sample_rate={RATE} block_frames={BLOCK} warmup_blocks={WARMUP} measured_blocks={BLOCKS} elapsed_s={elapsed:.6} real_time_factor={:.6} p99_us={:.3} max_us={:.3}",
            mode,
            elapsed / rendered_seconds,
            p99 as f64 / 1000.0,
            maximum as f64 / 1000.0
        );
    }
    Ok(())
}
