//! Compensated cold-track publication through the real prepared callback.
use super::*;

#[test]
fn cold_track_add_and_remove_preserve_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for remove in [false, true] {
        let mut editor = Editor::new(Project::new(RATE)?, 8)?;
        let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "memory/source.wav".into(),
            sample_rate: RATE,
            frames: 4096,
        })?
        else {
            return Err("expected source track".into());
        };
        editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.limiter",
            ))?,
        })?;
        let empty = if remove {
            let Created::Track(id) = editor.execute(Command::AddTrack {
                name: "empty".into(),
            })?
            else {
                return Err("expected empty track".into());
            };
            Some(id)
        } else {
            None
        };
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
        let table = Arc::new(SineTable::new());
        let first = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::clone(&table),
            None,
        )?);
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
        oracle.apply(PlaybackCommand::Play);
        let (mut control, mut callback) = prepared::channel(RATE)?;
        control
            .publish(Playback::try_from_prepared_with_session(first)?)
            .map_err(|_| "initial publication")?;
        control.command(PlaybackCommand::Play).map_err(|_| "play")?;
        let mut actual = [[0.0; 2]; 512];
        let mut expected = actual;
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected);
        if let Some(id) = empty {
            editor.execute(Command::RemoveTrack(id))?;
        } else {
            editor.execute(Command::AddTrack {
                name: "empty".into(),
            })?;
        }
        let next = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            table,
            None,
        )?);
        control
            .publish(Playback::try_from_prepared_with_session(next)?)
            .map_err(|_| "updated publication")?;
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected, "remove={remove}");
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        control.request_shutdown();
        measured_shutdown(&mut callback, &mut actual);
        control.reclaim();
    }
    Ok(())
}

#[test]
fn unused_return_add_and_remove_preserve_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for remove in [false, true] {
        unused_return_case(remove)?;
    }
    Ok(())
}

fn unused_return_case(remove: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "memory/source.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("expected source track".into());
    };
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    let empty = if remove {
        let Created::ReturnTrack(id) = editor.execute(Command::AddReturnTrack {
            name: "unused".into(),
        })?
        else {
            return Err("expected empty return".into());
        };
        Some(id)
    } else {
        None
    };
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
    let table = Arc::new(SineTable::new());
    let first = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(first)?)
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    if let Some(id) = empty {
        editor.execute(Command::RemoveReturnTrack(id))?;
    } else {
        editor.execute(Command::AddReturnTrack {
            name: "unused".into(),
        })?;
    }
    let next = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        table,
        None,
    )?);
    control
        .publish(Playback::try_from_prepared_with_session(next)?)
        .map_err(|_| "updated publication")?;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn earlier_unused_return_remove_and_undo_preserve_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "memory/source.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("expected source track".into());
    };
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    let Created::ReturnTrack(unused) = editor.execute(Command::AddReturnTrack {
        name: "unused".into(),
    })?
    else {
        return Err("expected unused return".into());
    };
    let Created::ReturnTrack(active) = editor.execute(Command::AddReturnTrack {
        name: "active".into(),
    })?
    else {
        return Err("expected active return".into());
    };
    editor.execute(Command::SetReturnSend {
        track,
        return_track: active,
        gain: 0.5,
    })?;
    let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
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
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    editor.execute(Command::RemoveReturnTrack(unused))?;
    for restore in [false, true] {
        if restore {
            assert!(editor.undo());
        }
        let next = prepare(editor.project())?;
        control
            .publish(Playback::try_from_prepared_with_session(next)?)
            .map_err(|_| "updated publication")?;
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected, "restore={restore}");
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.0));
        control.reclaim();
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}
