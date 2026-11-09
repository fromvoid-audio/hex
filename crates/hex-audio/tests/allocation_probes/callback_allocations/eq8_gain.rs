//! EQ8 uses ordinary publication, shared lane evaluation and runtime custody.
use super::*;
use hex_devices::eq8::{EQ8_TYPE_ID, parameter as p};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

#[test]
fn eq8_gain_automation_callback_publication_history_and_save_reopen() -> Result<(), Box<dyn Error>>
{
    eq8_gain_callback(false)
}

#[test]
fn eq8_gain_modulation_callback_with_automation_publication_is_allocation_free()
-> Result<(), Box<dyn Error>> {
    eq8_gain_callback(true)
}

fn eq8_gain_callback(with_modulation: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "eq8_gain automation".into(),
            path: "memory/eq8_gain.wav".into(),
            source_sample_rate: RATE,
            source_frames: (BLOCK * 3) as u64,
            playback_frames: (BLOCK * 3) as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("expected audio track".into());
    };
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: {
            use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
            let state = hex_engine::default_native_device_state(EQ8_TYPE_ID)?;
            let state = hex_devices::eq8::EQ8_DEFINITION.patch_parameter(
                &state,
                p::enabled(0, 1),
                ParameterValue::Bool(true),
            )?;
            hex_devices::eq8::EQ8_DEFINITION.patch_parameter(
                &state,
                p::gain(0, 1),
                ParameterValue::Float(6.0),
            )?
        },
    })?
    else {
        return Err("expected eq8_gain".into());
    };
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.0001, -0.0001]; BLOCK * 3])?)]);
    let lane = |value| -> Result<Command, hex_project::Invalid> {
        Ok(Command::ReplaceDeviceAutomation {
            device,
            parameter: p::OUTPUT_GAIN,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(value)?,
                outgoing: AutomationInterpolation::Linear,
            }],
        })
    };
    editor.execute(lane(1.0 / 16.0)?)?; // unity preserves the initial canonical gain.
    if with_modulation {
        use hex_devices::modulation::lfo::{Rate, Settings};
        use hex_project::{
            DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ModulationAmount,
            ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
            ModulationSourceId, Modulator,
        };
        let identity = editor.project().next_identity();
        let source = ModulationSourceId::new(identity)?;
        let modulation = DeviceModulation::new(
            vec![Modulator {
                id: source,
                enabled: true,
                color: Default::default(),
                state: Settings {
                    rate: Rate::Hertz(12_000.0),
                    ..Settings::default()
                }
                .encode()?,
            }],
            vec![DeviceModulationRoute {
                id: ModulationRouteId::new(identity + 1)?,
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: EQ8_TYPE_ID,
                    target: DeviceAutomationTarget::Parameter(p::OUTPUT_GAIN),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(0.015625)?,
                range: ModulationRange::default(),
                enabled: true,
            }],
        )?;
        editor.execute(Command::ReplaceDeviceModulation { device, modulation })?;
    }
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
        Ok(Arc::new(PreparedRevision::prepare(
            project,
            &media,
            Arc::clone(&table),
            None,
        )?))
    };
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut output = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    assert!(output.iter().flatten().any(|v| v.abs() > 0.0));
    editor.execute(lane(2.0 / 16.0)?)?;
    assert!(editor.undo());
    assert!(editor.redo());
    for bank in 0..5 {
        for band in 0..8 {
            for parameter in [
                p::enabled(bank, band),
                p::kind(bank, band),
                p::frequency(bank, band),
                p::gain(bank, band),
                p::q(bank, band),
            ] {
                let is_enable = parameter == p::enabled(bank, band);
                editor.execute(Command::ReplaceDeviceAutomation {
                    device,
                    parameter,
                    points: vec![
                        AutomationPoint {
                            beat: BeatTime::ZERO,
                            value: AutomationValue::new(if is_enable { 0.0 } else { 0.4 })?,
                            outgoing: if is_enable {
                                AutomationInterpolation::Step
                            } else {
                                AutomationInterpolation::Linear
                            },
                        },
                        AutomationPoint {
                            beat: editor.project().tempo_map().frame_to_beat(
                                (if is_enable { BLOCK * 3 / 2 } else { BLOCK * 2 }) as u64,
                            )?,
                            value: AutomationValue::new(if is_enable { 1.0 } else { 0.6 })?,
                            outgoing: AutomationInterpolation::Step,
                        },
                    ],
                })?;
            }
        }
    }
    editor.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: p::MODE,
        points: vec![
            AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5)?,
                outgoing: AutomationInterpolation::Step,
            },
            AutomationPoint {
                beat: editor
                    .project()
                    .tempo_map()
                    .frame_to_beat((BLOCK * 2 - 4) as u64)?,
                value: AutomationValue::new(0.0)?,
                outgoing: AutomationInterpolation::Step,
            },
            AutomationPoint {
                beat: editor
                    .project()
                    .tempo_map()
                    .frame_to_beat((BLOCK * 2 - 2) as u64)?,
                value: AutomationValue::new(1.0)?,
                outgoing: AutomationInterpolation::Step,
            },
        ],
    })?;
    let path = std::env::temp_dir().join(format!(
        "hex-eq8_gain-automation-{}-{with_modulation}.hex",
        std::process::id()
    ));
    hex_storage::save_new(&path, editor.project())?;
    let reopened = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let reopened = reopened?;
    assert_eq!(&reopened, editor.project());
    let next = prepare(&reopened)?;
    control
        .publish(Playback::try_from_prepared_with_session(next)?)
        .map_err(|_| "handoff")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|s| s.is_finite()));
    assert!(output.iter().flatten().any(|s| s.abs() > 0.0));
    control.reclaim();
    editor.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "tail publication")?;
    measured_block(&mut callback, &mut output);
    // The removed EQ retains the active fade and pending final routing. Its
    // ringing transition adds to this still-playing dry source. The final
    // incoming routing starts from empty history after removal, then the old
    // branches are cleared at the bounded fade endpoints.
    assert!(
        output
            .iter()
            .any(|v| (v[0] - 0.0001).abs() > 1e-10 || (v[1] + 0.0001).abs() > 1e-10)
    );
    // By this block both fades have ended and the source has ended too.
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|v| v.is_finite()));
    assert!(output.iter().flatten().all(|v| *v == 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
