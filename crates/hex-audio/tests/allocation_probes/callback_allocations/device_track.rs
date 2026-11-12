//! Zero-latency device edits while another track needs compensation.
use super::*;

#[test]
fn zero_latency_device_edits_keep_track_pdc_pcm_and_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported {
        track: delayed,
        asset: a,
        ..
    } = editor.execute(Command::ImportAudioTrack {
        name: "delayed".into(),
        path: "memory/a.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("delayed track".into());
    };
    let Created::Imported {
        track: edited,
        asset: b,
        ..
    } = editor.execute(Command::ImportAudioTrack {
        name: "edited".into(),
        path: "memory/b.wav".into(),
        sample_rate: RATE,
        frames: 4096,
    })?
    else {
        return Err("edited track".into());
    };
    editor.execute(Command::InsertDevice {
        track: delayed,
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?,
    })?;
    let media = BTreeMap::from([
        (a, Audio::new(RATE, vec![[0.25, -0.125]; 4096])?),
        (b, Audio::new(RATE, vec![[0.125, -0.0625]; 4096])?),
    ]);
    let table = Arc::new(SineTable::new());
    let first = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        None,
    )?);
    assert!(first.arrangement().main_latency_frames() > 0);
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
    let Created::Device(eq) = editor.execute(Command::InsertDevice {
        track: edited,
        before: None,
        state: hex_devices::eq::EqState::default().encode()?,
    })?
    else {
        return Err("eq device".into());
    };
    for remove in [false, true] {
        if remove {
            editor.execute(Command::RemoveDevice(eq))?;
        }
        let revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::clone(&table),
            None,
        )?);
        control
            .publish(Playback::try_from_prepared_with_session(revision)?)
            .map_err(|_| "updated publication")?;
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected, "remove={remove}");
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}
