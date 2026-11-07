//! Compressor uses ordinary publication, shared lane evaluation and runtime custody.
use super::*;
use hex_devices::eq8::{EQ8_TYPE_ID, parameter as p};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

#[test]
fn eq8_gain_automation_callback_publication_history_and_save_reopen() -> Result<(), Box<dyn Error>>
{
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
        state: hex_engine::default_native_device_state(EQ8_TYPE_ID)?,
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
    assert!((output[0][0] - 0.0001).abs() < 1e-10);
    assert_eq!(output[BLOCK - 1], [0.0001, -0.0001]);
    editor.execute(lane(2.0 / 16.0)?)?;
    assert!(editor.undo());
    assert!(editor.redo());
    let path = std::env::temp_dir().join(format!(
        "hex-eq8_gain-automation-{}.hex",
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
    assert!(output.iter().all(|s| s[0].is_finite() && s[0] > 0.0));
    assert!((output[BLOCK - 1][0] - 0.0002).abs() < 1e-10);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
