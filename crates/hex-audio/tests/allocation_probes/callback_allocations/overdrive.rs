//! Overdrive uses ordinary publication, shared lane evaluation and runtime custody.
use super::*;
use hex_devices::overdrive::{OVERDRIVE_TYPE_ID, parameter as p};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

#[test]
fn overdrive_automation_callback_publication_history_and_save_reopen() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } =
        editor.execute(Command::ImportPreparedAudioTrack {
            name: "overdrive automation".into(),
            path: "memory/overdrive.wav".into(),
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
        state: hex_engine::default_native_device_state(OVERDRIVE_TYPE_ID)?,
    })?
    else {
        return Err("expected overdrive".into());
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
    assert!(output[..11].iter().all(|s| *s == [0.0; 2]));
    assert_eq!(output[BLOCK - 1], [0.0001, -0.0001]);
    editor.execute(lane(0.5)?)?;
    assert!(editor.undo());
    assert!(editor.redo());
    // The next callback changes all coefficient groups at sample density,
    // including exp/pow conversion for frequency, bandwidth, drive and tone.
    let end = editor
        .project()
        .tempo_map()
        .frame_to_beat((BLOCK * 3) as u64)?;
    for parameter in [p::FREQUENCY, p::BANDWIDTH, p::DRIVE, p::TONE, p::DYNAMICS] {
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
    let path = std::env::temp_dir().join(format!(
        "hex-overdrive-automation-{}.hex",
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
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
