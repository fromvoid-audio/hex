//! Reverb uses ordinary publication, shared lane evaluation and runtime custody.
use super::*;
use hex_devices::reverb::{REVERB_TYPE_ID, parameter as p};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

#[test]
fn reverb_output_automation_callback_publication_history_and_save_reopen()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "reverb_outputs automation".into(),
            path: "memory/reverb_outputs.wav".into(),
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
            let mut state = hex_engine::default_native_device_state(REVERB_TYPE_ID)?;
            for parameter in [p::LO_CUT, p::HI_CUT, p::HIGH, p::LOW] {
                state = hex_devices::reverb::REVERB_DEFINITION.patch_parameter(
                    &state,
                    parameter,
                    ParameterValue::Bool(true),
                )?;
            }
            state
        },
    })?
    else {
        return Err("expected reverb_outputs".into());
    };
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.0001, -0.0001]; BLOCK * 3])?)]);
    let lane = |value| -> Result<Command, hex_project::Invalid> {
        Ok(Command::ReplaceDeviceAutomation {
            device,
            parameter: p::DRY_WET,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(value)?,
                outgoing: AutomationInterpolation::Linear,
            }],
        })
    };
    editor.execute(lane(0.0)?)?; // the existing mix ramp reaches fully dry.
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
    assert_eq!(output[0], [0.0001, -0.0001]);
    assert_eq!(output[BLOCK - 1], [0.0001, -0.0001]);
    editor.execute(lane(0.5)?)?;
    assert!(editor.undo());
    assert!(editor.redo());
    // Change room outputs at sample density through the real callback.
    let end = editor
        .project()
        .tempo_map()
        .frame_to_beat((BLOCK * 3) as u64)?;
    for parameter in [
        p::DECAY,
        p::SIZE,
        p::HIGH_RATIO,
        p::LOW_RATIO,
        p::DIFFUSION,
        p::DENSITY,
        p::LO_FREQ,
        p::HI_FREQ,
        p::HIGH_FREQ,
        p::LOW_FREQ,
        p::LO_Q,
        p::HI_Q,
        p::PREDELAY,
        p::SPIN_AMOUNT,
        p::SPIN_RATE,
        p::SHAPE,
        p::SCALE,
        p::CHORUS_AMOUNT,
        p::CHORUS_RATE,
        p::STEREO,
        p::REFLECT,
        p::DIFFUSE,
    ] {
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.1)?,
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: end,
                    value: AutomationValue::new(0.9)?,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })?;
    }
    // Native recorded touches carry no VST3 class stamp in schema79.
    // Exercise the current take command, history and half-open fallback before
    // reopening and delivering this take through the actual callback.
    let start = editor.project().tempo_map().frame_to_beat(BLOCK as u64)?;
    let middle = editor
        .project()
        .tempo_map()
        .frame_to_beat((BLOCK + BLOCK / 2) as u64)?;
    let finish = editor
        .project()
        .tempo_map()
        .frame_to_beat((BLOCK * 2) as u64)?;
    let before_take = editor.project().clone();
    let base_end = before_take
        .device(device)
        .ok_or("missing reverb")?
        .automation_lane(p::DECAY)
        .ok_or("missing decay lane")?
        .value_at(finish);
    editor.execute(Command::SpliceDeviceAutomationTouch {
        device,
        target: hex_project::DeviceAutomationTarget::Parameter(p::DECAY),
        vst3_class: None,
        spans: vec![hex_project::DeviceAutomationTouchSpan {
            start,
            end: finish,
            steps: vec![
                hex_project::DeviceAutomationTouchStep {
                    beat: start,
                    value: AutomationValue::new(0.75)?,
                },
                hex_project::DeviceAutomationTouchStep {
                    beat: middle,
                    value: AutomationValue::new(0.25)?,
                },
            ],
        }],
    })?;
    let lane = editor
        .project()
        .device(device)
        .ok_or("missing reverb")?
        .automation_lane(p::DECAY)
        .ok_or("missing recorded lane")?;
    assert_eq!(lane.vst3_class, None);
    assert_eq!(lane.value_at(start), Some(AutomationValue::new(0.75)?));
    assert_eq!(lane.value_at(middle), Some(AutomationValue::new(0.25)?));
    assert_eq!(
        lane.value_at(finish),
        base_end,
        "touch end resumes the base curve"
    );
    let recorded = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &before_take);
    assert!(editor.redo());
    assert_eq!(editor.project(), &recorded);
    let path = std::env::temp_dir().join(format!(
        "hex-reverb_outputs-automation-{}.hex",
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
    let targets: Vec<_> = editor
        .project()
        .device(device)
        .ok_or("missing reverb")?
        .automation()
        .iter()
        .map(|lane| lane.target)
        .collect();
    for target in targets {
        let hex_project::DeviceAutomationTarget::Parameter(parameter) = target else {
            return Err("expected provider lane".into());
        };
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![],
        })?;
    }
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "canonical handoff")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().all(|s| s.is_finite()));
    control.reclaim();
    editor.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "tail handoff")?;
    measured_block(&mut callback, &mut output);
    assert!(
        output
            .iter()
            .flatten()
            .all(|s| s.is_finite() && s.abs() < 1.0)
    );
    // The source clip has ended: only the retained canonical room is audible.
    assert!(output.iter().flatten().any(|s| s.abs() > 1e-10));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
