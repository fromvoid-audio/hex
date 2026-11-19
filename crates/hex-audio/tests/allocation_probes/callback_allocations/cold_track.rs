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
    verify_silent_track_latency_edit(
        SilentLatencyRouting::TrackReverb,
        IsolatedLatencyEdit::Insert,
    )
}

#[test]
fn isolated_latency_insert_preserves_group_and_return_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::GroupAndReturn,
        IsolatedLatencyEdit::Insert,
    )
}

#[test]
fn isolated_latency_insert_preserves_held_operator_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::HeldOperator,
        IsolatedLatencyEdit::Insert,
    )
}

#[test]
fn isolated_latency_removal_preserves_group_and_return_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::GroupAndReturn,
        IsolatedLatencyEdit::Remove,
    )
}

#[test]
fn isolated_latency_removal_preserves_held_operator_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::HeldOperator,
        IsolatedLatencyEdit::Remove,
    )
}

#[test]
fn buffered_latency_insert_refuses_without_callback_allocations_or_source_replacement()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::GroupAndReturn,
        IsolatedLatencyEdit::BufferedInsert,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SilentLatencyRouting {
    TrackReverb,
    GroupAndReturn,
    HeldOperator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IsolatedLatencyEdit {
    Insert,
    Remove,
    BufferedInsert,
}

fn verify_silent_track_latency_edit(
    routing: SilentLatencyRouting,
    edit: IsolatedLatencyEdit,
) -> Result<(), Box<dyn Error>> {
    use hex_audio::prepared::{AppliedAudioSource, PublicationPolicy};
    use hex_project::BeatTime;

    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    let names = if routing == SilentLatencyRouting::GroupAndReturn {
        &["edited", "limiter", "sibling"][..]
    } else {
        &["edited", "unchanged"][..]
    };
    for (index, &name) in names.iter().enumerate() {
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
        if index == 0 && edit == IsolatedLatencyEdit::BufferedInsert {
            samples.fill([0.25, -0.125]);
        } else if routing != SilentLatencyRouting::HeldOperator {
            samples[..512].fill([0.25, -0.125]);
        }
        media.insert(asset, Audio::new(RATE, samples)?);
        tracks.push(track);
    }
    if routing == SilentLatencyRouting::TrackReverb {
        editor.execute(Command::Mute {
            track: tracks[0],
            muted: true,
        })?;
    }
    let insert_limiter = |track| -> Result<Command, Box<dyn Error>> {
        Ok(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.limiter",
            ))?,
        })
    };
    editor.execute(insert_limiter(tracks[1])?)?;
    let reverb = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        ParameterId::from_static(2),
        ParameterValue::Float(100.0),
    )?;
    if routing == SilentLatencyRouting::GroupAndReturn {
        let Created::Track(group) = editor.execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&tracks[1..])?,
            name: "group".into(),
        })?
        else {
            return Err("expected group".into());
        };
        let Created::ReturnTrack(bus) = editor.execute(Command::AddReturnTrack {
            name: "room".into(),
        })?
        else {
            return Err("expected return".into());
        };
        editor.execute(Command::SetReturnSend {
            track: tracks[1],
            return_track: bus,
            gain: 0.5,
        })?;
        editor.execute(Command::InsertDevice {
            track: group,
            before: None,
            state: reverb.clone(),
        })?;
        editor.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: reverb,
        })?;
    } else if routing == SilentLatencyRouting::TrackReverb {
        editor.execute(Command::InsertDevice {
            track: tracks[1],
            before: None,
            state: reverb,
        })?;
    } else {
        editor.execute(Command::InsertInstrument {
            track: tracks[1],
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.operator"),
            )?,
        })?;
        let Created::Clip(clip) = editor.execute(Command::PlaceMidiClip {
            track: tracks[1],
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1)?,
        })?
        else {
            return Err("expected operator clip".into());
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(17, 64)?,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })?;
    }
    let command = if edit == IsolatedLatencyEdit::Remove {
        let Created::Device(device) = editor.execute(insert_limiter(tracks[0])?)? else {
            return Err("expected limiter".into());
        };
        Command::RemoveDevice(device)
    } else {
        insert_limiter(tracks[0])?
    };
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        PreparedRevision::prepare(project, &media, Arc::clone(&table), None).map(Arc::new)
    };
    let first = prepare(editor.project())?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    oracle.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control.configure_device_resource_publication_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        },
    )?;
    let context = hex_engine::AudioSourceContext {
        project_generation: 1,
        editor_revision: 0,
    };
    let initial = control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(first)?,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    for _ in 0..12 {
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected);
    }
    let active = control
        .status
        .recording_source()
        .ok_or("missing original source")?;
    assert_eq!((active.context, active.audio_revision), (context, initial));
    editor.execute(command)?;
    let mut next = Playback::try_from_prepared_with_session(prepare(editor.project())?)?;
    next.require_continuous_device_insert();
    let context = hex_engine::AudioSourceContext {
        editor_revision: 1,
        ..context
    };
    let submitted = control
        .publish_with_admission_and_source_on_control(
            next,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "updated publication")?;
    measured_block(&mut callback, &mut actual);
    if edit == IsolatedLatencyEdit::BufferedInsert {
        assert_eq!(control.status.recording_source(), Some(active));
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                submitted,
                hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
            ))
        );
    } else {
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                context,
                audio_revision: submitted,
                ..active
            })
        );
        assert!(control.status.rejected_revision().is_none());
    }
    oracle.render(&mut expected)?;
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(actual, expected);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}
