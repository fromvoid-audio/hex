//! Return-tail custody across a refused compensated restart.
use super::*;

#[test]
fn active_return_tail_survives_refused_restart_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "memory/source.wav".into(),
        sample_rate: RATE,
        frames: 12_000,
    })?
    else {
        return Err("expected source track".into());
    };
    let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
        name: "room".into(),
    })?
    else {
        return Err("expected return".into());
    };
    editor.execute(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 1.0,
    })?;
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        ParameterId::from_static(2),
        ParameterValue::Float(100.0),
    )?;
    let Created::Device(device) = editor.execute(Command::InsertReturnDevice {
        return_track: bus,
        before: None,
        state,
    })?
    else {
        return Err("expected device".into());
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples)?)]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        PreparedRevision::prepare(project, &media, Arc::clone(&table), None).map(Arc::new)
    };
    let first = prepare(editor.project())?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(first)?)
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    for _ in 0..12 {
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected);
    }
    editor.execute(Command::RemoveDevice(device))?;
    let next = prepare(editor.project())?;
    let mut tail_oracle = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
    tail_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "oracle tail handoff")?;
    control
        .publish(Playback::try_from_prepared_with_session(next)?)
        .map_err(|_| "tail publication")?;
    measured_block(&mut callback, &mut actual);
    tail_oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    assert!(control.status.rejected_revision().is_none());
    control.reclaim();
    editor.execute(Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "latency publication")?;
    measured_block(&mut callback, &mut actual);
    tail_oracle.render(&mut expected)?;
    assert_eq!(
        control.status.rejected_revision().map(|(_, reason)| reason),
        Some(hex_engine::playback::PlaybackHandoffRejection::LatencyLayoutChanged),
    );
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn active_return_removal_refuses_reset_without_callback_allocations() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "memory/source.wav".into(),
        sample_rate: RATE,
        frames: 12_000,
    })?
    else {
        return Err("expected source track".into());
    };
    let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
        name: "room".into(),
    })?
    else {
        return Err("expected return".into());
    };
    editor.execute(Command::SetReturnSend {
        track,
        return_track: bus,
        gain: 1.0,
    })?;
    let state = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        ParameterId::from_static(2),
        ParameterValue::Float(100.0),
    )?;
    let Created::Device(device) = editor.execute(Command::InsertReturnDevice {
        return_track: bus,
        before: None,
        state,
    })?
    else {
        return Err("expected device".into());
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples)?)]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        PreparedRevision::prepare(project, &media, Arc::clone(&table), None).map(Arc::new)
    };
    editor.execute(Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    let first = prepare(editor.project())?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(first)?)
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    for _ in 0..12 {
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected);
    }
    editor.execute(Command::RemoveDevice(device))?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "removed return publication")?;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    assert_eq!(
        control.status.rejected_revision().map(|(_, reason)| reason),
        Some(hex_engine::playback::PlaybackHandoffRejection::LatencyLayoutChanged),
    );
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}
