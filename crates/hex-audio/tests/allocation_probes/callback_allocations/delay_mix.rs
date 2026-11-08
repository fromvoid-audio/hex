//! Actual owned Delay publication, rollback custody, dense delivery and retained tail.
use super::*;
use hex_devices::delay::{self as p, DEFINITION as D};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};
#[test]
fn delay_mix_automation_callback_publication_history_and_save_reopen() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "delay mix".into(),
            path: "memory/delay-mix.wav".into(),
            source_sample_rate: RATE,
            source_frames: (BLOCK * 3) as u64,
            playback_frames: (BLOCK * 3) as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })?
    else {
        return Err("audio track".into());
    };
    let mut state = D.default_state()?;
    for (id, value) in [
        (p::LEFT_SYNC, ParameterValue::Bool(false)),
        (p::TIME, ParameterValue::Float(1.0)),
        (p::FILTER, ParameterValue::Bool(false)),
        (p::HIGH_QUALITY, ParameterValue::Bool(false)),
        (p::DRY_WET, ParameterValue::Float(0.4)),
    ] {
        state = D.patch_parameter(&state, id, value)?;
    }
    let Created::Device(device) = editor.execute(Command::InsertDevice {
        track,
        before: None,
        state,
    })?
    else {
        return Err("delay".into());
    };
    let lane = |parameter, value| -> Result<Command, hex_project::Invalid> {
        Ok(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(value)?,
                outgoing: AutomationInterpolation::Step,
            }],
        })
    };
    editor.execute(lane(p::FEEDBACK, 0.5 / 0.95)?)?;
    editor.execute(lane(p::DRY_WET, 0.4)?)?;
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.0001, -0.0001]; BLOCK * 3])?)]);
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
    let mut out = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut out);
    assert!(out.iter().flatten().all(|v| v.is_finite()));
    assert!(out.iter().flatten().any(|v| v.abs() > 0.0));
    let previous_output = out[BLOCK - 1][0];
    for (parameter, a, b) in [
        (p::FEEDBACK, 0.2 / 0.95, 0.8 / 0.95),
        (p::DRY_WET, 0.1, 0.9),
    ] {
        editor.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(a)?,
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: editor
                        .project()
                        .tempo_map()
                        .frame_to_beat((BLOCK * 2) as u64)?,
                    value: AutomationValue::new(b)?,
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })?;
    }
    assert!(editor.undo());
    assert!(editor.redo());
    let path = std::env::temp_dir().join(format!(
        "hex-delay-mix-automation-{}.hex",
        std::process::id()
    ));
    hex_storage::save_new(&path, editor.project())?;
    let loaded = hex_storage::load(&path);
    std::fs::remove_file(path)?;
    let loaded = loaded?;
    assert_eq!(&loaded, editor.project());
    control
        .publish(Playback::try_from_prepared_with_session(prepare(&loaded)?)?)
        .map_err(|_| "handoff")?;
    measured_block(&mut callback, &mut out);
    assert!(out.iter().flatten().all(|v| v.is_finite()));
    assert!(out.iter().flatten().any(|v| v.abs() > 0.0));
    // A retained old graph would remain at its previous steady output. The
    // new feedback/mix ramp must be audible in this actual callback.
    assert!(out[BLOCK - 1][0] > previous_output + 1e-6);
    control.reclaim();
    editor.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "tail")?;
    measured_block(&mut callback, &mut out);
    measured_block(&mut callback, &mut out);
    assert!(out.iter().flatten().all(|v| v.is_finite()));
    assert!(out.iter().flatten().any(|v| v.abs() > 1e-20));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut out);
    control.reclaim();
    Ok(())
}
