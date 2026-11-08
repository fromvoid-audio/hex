//! Optimized, headless component timing for one Utility per track. This excludes
//! native APIs, streaming, worker publication and physical output deadlines.
use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice::{NativeTransientOperation, NativeTransientValue, PreparedNativeProcessor};
use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata, ParameterValue};
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
    for mode in [
        "static",
        "armed_idle",
        "armed_held",
        "gain_curve",
        "all_curves",
    ] {
        let automated = matches!(mode, "gain_curve" | "all_curves");
        let armed = matches!(mode, "armed_idle" | "armed_held");
        let mut first_device = None;
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
                state: hex_engine::default_native_device_state(UTILITY_TYPE_ID)?,
            })?
            else {
                return Err("unexpected device result".into());
            };
            first_device.get_or_insert(device);
            if mode == "all_curves" {
                let state = UTILITY_DEFINITION.default_state()?;
                for descriptor in UTILITY_DEFINITION.descriptors() {
                    if descriptor.id == parameter::GAIN {
                        continue;
                    }
                    let semantics = UTILITY_DEFINITION
                        .parameter_semantics_for_state(&state, descriptor.id)?
                        .ok_or("undeclared utility parameter")?;
                    let normalized = semantics.normalization.normalize(
                        *descriptor,
                        UTILITY_DEFINITION.read_parameter(&state, descriptor.id)?,
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
            if automated {
                editor.execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter: parameter::GAIN,
                    points: vec![
                        AutomationPoint {
                            beat: BeatTime::ZERO,
                            value: AutomationValue::new(0.25 / MAX_UTILITY_GAIN)?,
                            outgoing: AutomationInterpolation::Linear,
                        },
                        AutomationPoint {
                            beat: BeatTime::from_quarters(4)?,
                            value: AutomationValue::new(0.75 / MAX_UTILITY_GAIN)?,
                            outgoing: AutomationInterpolation::Step,
                        },
                    ],
                })?;
            }
        }
        let revision = Arc::new(if armed {
            PreparedRevision::prepare_for_native_parameter_input(
                editor.project(),
                &media,
                Arc::new(SineTable::new()),
                None,
                first_device.ok_or("armed profile requires a device")?,
                DeviceAutomationTarget::Parameter(parameter::GAIN),
            )?
        } else {
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)?
        });
        // Control-side layout accounting, outside warmup and measured scopes.
        // Exact flat backing and concrete twin payloads; this deliberately does
        // not claim complete graph/asset/allocator resident memory.
        let lanes = if mode == "all_curves" {
            14
        } else {
            usize::from(automated)
        };
        let points = if mode == "all_curves" {
            15
        } else if automated {
            2
        } else {
            0
        };
        let program_bytes =
            hex_lattice::PreparedParameterAutomation::quoted_storage_bytes(lanes, points)
                .ok_or("program storage overflow")?;
        let state = UTILITY_DEFINITION.default_state()?;
        let pair_bytes = if automated || armed {
            hex_devices::utility::UtilityDefinition::prepare_owned_automated_filter(&state, RATE)?
                .declared_runtime_bytes()
                .checked_mul(2)
                .ok_or("runtime storage overflow")?
        } else {
            0
        };
        println!(
            "layout mode={mode} authored_lanes_per_device={lanes} authored_points_per_device={points} authored_program_backing_per_device={program_bytes} concrete_runtime_pair_bytes={pair_bytes} runtime_pairs={} armed_provider_slots={} armed_program_backing={}",
            if automated {
                tracks
            } else {
                usize::from(armed)
            },
            usize::from(armed),
            if armed {
                hex_lattice::PreparedParameterAutomation::quoted_storage_bytes(1, 0)
                    .ok_or("armed program storage overflow")?
            } else {
                0
            },
        );
        let mut playback = SessionPlayback::new(revision)?;
        playback.apply(SessionPlaybackCommand::Play)?;
        if mode == "armed_held" {
            playback.set_native_parameter_input(NativeTransientOperation::Set(
                NativeTransientValue::Parameter {
                    id: parameter::GAIN,
                    value: ParameterValue::Float(1.0),
                },
            ))?;
        }
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
