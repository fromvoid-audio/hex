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
fn sounding_track_removal_preserves_other_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_sounding_track_removal(false)
}

#[test]
fn looped_sounding_track_removal_preserves_other_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_sounding_track_removal(true)
}

fn verify_sounding_track_removal(looped: bool) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "source".into(),
        path: "memory/source.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("expected source".into());
    };
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    if looped {
        editor.execute(Command::SetLoopRegion {
            start: 0,
            end: 384,
            enabled: true,
        })?;
    }
    let mut media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
    let table = Arc::new(SineTable::new());
    let reference = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let mut oracle = Playback::try_from_prepared_with_session(reference)?;
    oracle.apply(PlaybackCommand::Play);
    let Created::Imported {
        track: removed,
        asset: removed_asset,
        ..
    } = editor.execute(Command::ImportAudioTrack {
        name: "removed".into(),
        path: "memory/removed.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("expected removed track".into());
    };
    media.insert(removed_asset, Audio::new(RATE, vec![[0.125; 2]; 4096])?);
    let first = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(first)?)
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    editor.execute(Command::RemoveTrack(removed))?;
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
fn future_audio_track_add_preserves_pcm_without_callback_allocations() -> Result<(), Box<dyn Error>>
{
    verify_added_audio_track(AddedAudio::Future)
}

#[test]
fn ended_audio_track_add_preserves_pcm_without_callback_allocations() -> Result<(), Box<dyn Error>>
{
    verify_added_audio_track(AddedAudio::Ended)
}

#[test]
fn active_audio_track_add_preserves_pcm_without_callback_allocations() -> Result<(), Box<dyn Error>>
{
    verify_added_audio_track(AddedAudio::Active)
}

#[test]
fn looped_active_audio_track_add_preserves_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_added_audio_track(AddedAudio::LoopActive)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AddedAudio {
    Future,
    Ended,
    Active,
    LoopActive,
}

fn verify_added_audio_track(case: AddedAudio) -> Result<(), Box<dyn Error>> {
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
    if case == AddedAudio::LoopActive {
        editor.execute(Command::SetLoopRegion {
            start: 0,
            end: 4096,
            enabled: true,
        })?;
    }
    let mut media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
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
    let frames = if case == AddedAudio::Ended { 16 } else { 1024 };
    let Created::Imported {
        track: added,
        asset: added_asset,
        clip,
    } = editor.execute(Command::ImportAudioTrack {
        name: "later".into(),
        path: "memory/later.wav".into(),
        sample_rate: RATE,
        frames,
    })?
    else {
        return Err("expected added track".into());
    };
    if case == AddedAudio::Future {
        editor.execute(Command::MoveClip {
            clip,
            track: added,
            start: 2048,
        })?;
    }
    media.insert(
        added_asset,
        Audio::new(RATE, vec![[0.125; 2]; frames as usize])?,
    );
    let next = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        table,
        None,
    )?);
    let latency = next.arrangement().main_latency_frames() as usize;
    control
        .publish(Playback::try_from_prepared_with_session(next)?)
        .map_err(|_| "updated publication")?;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    if matches!(case, AddedAudio::Active | AddedAudio::LoopActive) {
        assert!(latency > 0 && latency < actual.len());
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            let added = if index < latency { 0.0 } else { 0.125 };
            assert_eq!(
                *actual,
                [expected[0] + added, expected[1] + added],
                "frame {index}"
            );
        }
    } else {
        assert_eq!(actual, expected);
    }
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn future_audio_track_during_loop_preserves_pcm_without_callback_allocations()
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
    editor.execute(Command::SetLoopRegion {
        start: 0,
        end: 2048,
        enabled: true,
    })?;
    let mut media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?)]);
    let table = Arc::new(SineTable::new());
    let first = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    let mut old_oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    old_oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(first)?)
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    measured_block(&mut callback, &mut actual);
    let mut old_expected = [[0.0; 2]; 512];
    old_oracle.render(&mut old_expected)?;
    assert_eq!(actual, old_expected);

    let Created::Imported {
        track: added,
        asset: added_asset,
        clip,
    } = editor.execute(Command::ImportAudioTrack {
        name: "later".into(),
        path: "memory/later.wav".into(),
        sample_rate: RATE,
        frames: 1024,
    })?
    else {
        return Err("expected added track".into());
    };
    editor.execute(Command::MoveClip {
        clip,
        track: added,
        start: 1024,
    })?;
    media.insert(added_asset, Audio::new(RATE, vec![[0.125; 2]; 1024])?);
    let next = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        table,
        None,
    )?);
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&next))?;
    oracle.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 512];
    oracle.render(&mut expected)?;
    assert_eq!(actual, expected);
    control
        .publish(Playback::try_from_prepared_with_session(next)?)
        .map_err(|_| "updated publication")?;
    let mut added_is_audible = false;
    for _ in 0..4 {
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        old_oracle.render(&mut old_expected)?;
        added_is_audible |= expected != old_expected;
        assert_eq!(actual, expected);
    }
    assert!(added_is_audible, "the added track must reach the output");
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
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

#[test]
fn silent_track_latency_addition_preserves_other_reverb_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    for name in ["muted", "reverb"] {
        let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: name.into(),
            path: "memory/source.wav".into(),
            sample_rate: RATE,
            frames: 12_000,
        })?
        else {
            return Err("expected source".into());
        };
        let mut samples = vec![[0.0; 2]; 12_000];
        samples[..512].fill([0.25, -0.125]);
        media.insert(asset, Audio::new(RATE, samples)?);
        tracks.push(track);
    }
    editor.execute(Command::Mute {
        track: tracks[0],
        muted: true,
    })?;
    editor.execute(Command::InsertDevice {
        track: tracks[1],
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    let reverb = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        ParameterId::from_static(2),
        ParameterValue::Float(100.0),
    )?;
    editor.execute(Command::InsertDevice {
        track: tracks[1],
        before: None,
        state: reverb,
    })?;
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
    editor.execute(Command::InsertDevice {
        track: tracks[0],
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    control
        .publish(Playback::try_from_prepared_with_session(prepare(
            editor.project(),
        )?)?)
        .map_err(|_| "updated publication")?;
    measured_block(&mut callback, &mut actual);
    oracle.render(&mut expected)?;
    assert!(control.status.rejected_revision().is_none());
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(actual, expected);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}
