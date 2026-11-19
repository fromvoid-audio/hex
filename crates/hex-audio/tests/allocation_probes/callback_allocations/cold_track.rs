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
fn edited_return_send_preserves_tail_and_source_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for edit in [
        IsolatedLatencyEdit::Insert,
        IsolatedLatencyEdit::Remove,
        IsolatedLatencyEdit::BufferedInsert,
        IsolatedLatencyEdit::BufferedRemove,
    ] {
        verify_silent_track_latency_edit(SilentLatencyRouting::EditedReturnSend, edit)?;
    }
    Ok(())
}

#[test]
fn buffered_latency_insert_refuses_without_callback_allocations_or_source_replacement()
-> Result<(), Box<dyn Error>> {
    verify_silent_track_latency_edit(
        SilentLatencyRouting::GroupAndReturn,
        IsolatedLatencyEdit::BufferedInsert,
    )
}

#[test]
fn approved_isolated_latency_edit_preserves_other_pcm_and_source_without_allocations()
-> Result<(), Box<dyn Error>> {
    for edit in [
        IsolatedLatencyEdit::ApprovedInsert,
        IsolatedLatencyEdit::ApprovedRemove,
    ] {
        verify_silent_track_latency_edit(SilentLatencyRouting::GroupAndReturn, edit)?;
    }
    Ok(())
}

#[test]
fn grouped_send_track_latency_edits_preserve_admitted_pcm_and_source_without_allocations()
-> Result<(), Box<dyn Error>> {
    for edit in [
        IsolatedLatencyEdit::Insert,
        IsolatedLatencyEdit::Remove,
        IsolatedLatencyEdit::ApprovedInsert,
        IsolatedLatencyEdit::ApprovedRemove,
    ] {
        verify_silent_track_latency_edit(SilentLatencyRouting::EditedGroupAndReturn, edit)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SilentLatencyRouting {
    TrackReverb,
    GroupAndReturn,
    EditedReturnSend,
    EditedGroupAndReturn,
    HeldOperator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IsolatedLatencyEdit {
    Insert,
    Remove,
    BufferedInsert,
    BufferedRemove,
    ApprovedInsert,
    ApprovedRemove,
}

fn verify_silent_track_latency_edit(
    routing: SilentLatencyRouting,
    edit: IsolatedLatencyEdit,
) -> Result<(), Box<dyn Error>> {
    use hex_audio::prepared::{AppliedAudioSource, PublicationPolicy, PublicationReceiptState};
    use hex_project::BeatTime;

    const BLOCK: usize = 512;
    const PREROLL_BLOCKS: usize = 12;
    const FADE: usize = 128;
    const PENDING_HIT_FRAMES: usize = 8;

    let approved = matches!(
        edit,
        IsolatedLatencyEdit::ApprovedInsert | IsolatedLatencyEdit::ApprovedRemove
    );
    let buffered = approved
        || matches!(
            edit,
            IsolatedLatencyEdit::BufferedInsert | IsolatedLatencyEdit::BufferedRemove
        );
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    let edited_grouped = routing == SilentLatencyRouting::EditedGroupAndReturn;
    let edited_sends = edited_grouped || routing == SilentLatencyRouting::EditedReturnSend;
    let grouped = edited_sends || routing == SilentLatencyRouting::GroupAndReturn;
    let cut_pending_hit = approved && edited_grouped;
    let mut reference_audio = None;
    let names = if grouped {
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
        if index == 0 && cut_pending_hit {
            samples[..BLOCK].fill([0.25, -0.125]);
            // The first hit is pending at the initial continuity check and
            // reaches the shared buses before the confirmed cut.
            let request = PREROLL_BLOCKS * BLOCK;
            samples[request - PENDING_HIT_FRAMES..request].fill([0.125, 0.375]);
            reference_audio = Some((asset, Audio::new(RATE, samples.clone())?));
            let cut = request + BLOCK + FADE;
            samples[cut - PENDING_HIT_FRAMES..cut].fill([-0.375, 0.5]);
        } else if index == 0 && buffered {
            // The right channel is an independent witness for untouched DSP.
            samples.fill([0.25, 0.0]);
        } else if routing != SilentLatencyRouting::HeldOperator && !(edited_sends && index == 1) {
            samples[..BLOCK].fill([0.25, -0.125]);
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
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.limiter"))?;
        let state = if cut_pending_hit {
            // Keep the pending hit beyond the fade-in so its absence is
            // visible directly in the resumed PCM comparison.
            hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(6),
                ParameterValue::Choice(2),
            )?
        } else {
            state
        };
        Ok(Command::InsertDevice {
            track,
            before: None,
            state,
        })
    };
    editor.execute(insert_limiter(tracks[1])?)?;
    let reverb = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        ParameterId::from_static(2),
        ParameterValue::Float(if cut_pending_hit { 50.0 } else { 100.0 }),
    )?;
    if grouped {
        let Created::Track(group) = editor.execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&tracks[usize::from(!edited_grouped)..])?,
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
        if edited_sends {
            editor.execute(Command::SetReturnSend {
                track: tracks[0],
                return_track: bus,
                gain: 0.5,
            })?;
        }
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
    let command = if matches!(
        edit,
        IsolatedLatencyEdit::Remove
            | IsolatedLatencyEdit::BufferedRemove
            | IsolatedLatencyEdit::ApprovedRemove
    ) {
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
    if cut_pending_hit {
        let latency = first.arrangement().main_latency_frames() as usize;
        assert!((FADE + PENDING_HIT_FRAMES..BLOCK).contains(&latency));
    }
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first))?;
    oracle.apply(PlaybackCommand::Play);
    // This independently prepared predecessor never received the pending
    // hit. Its previously admitted bus state is the post-cut PCM oracle.
    let mut admitted_only = if let Some((asset, audio)) = reference_audio {
        let mut original_media = media.clone();
        original_media.insert(asset, audio);
        let revision =
            PreparedRevision::prepare(editor.project(), &original_media, Arc::clone(&table), None)?;
        let mut playback = Playback::try_from_prepared_with_session(Arc::new(revision))?;
        playback.apply(PlaybackCommand::Play);
        Some(playback)
    } else {
        None
    };
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
    let mut actual = [[0.0; 2]; BLOCK];
    let mut expected = actual;
    let mut admitted_pcm = actual;
    for _ in 0..PREROLL_BLOCKS {
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(actual, expected);
        if let Some(admitted) = admitted_only.as_mut() {
            admitted.render(&mut admitted_pcm)?;
            assert_eq!(actual, admitted_pcm);
        }
    }
    let active = control
        .status
        .recording_source()
        .ok_or("missing original source")?;
    assert_eq!((active.context, active.audio_revision), (context, initial));
    editor.execute(command)?;
    let successor = prepare(editor.project())?;
    let mut next = Playback::try_from_prepared_with_session(Arc::clone(&successor))?;
    next.require_continuous_device_insert();
    let context = hex_engine::AudioSourceContext {
        editor_revision: 1,
        ..context
    };
    let policy = if approved {
        PublicationPolicy::DeviceInsert {
            predecessor: active,
            fence: control
                .capture_fresh_runtime_fence()
                .ok_or("device cut fence")?,
        }
    } else {
        PublicationPolicy::InheritRuntime
    };
    let submitted = control
        .publish_with_admission_and_source_on_control(next, policy, context, |_| Ok::<_, ()>(()))
        .map_err(|_| "updated publication")?;
    let position = control.status.position.load(Ordering::Acquire);
    measured_block(&mut callback, &mut actual);
    assert!(control.status.playing.load(Ordering::Acquire));
    assert_eq!(
        control.status.position.load(Ordering::Acquire),
        position + actual.len() as u64
    );
    if buffered {
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
    if let Some(admitted) = admitted_only.as_mut() {
        admitted.render(&mut admitted_pcm)?;
        assert_eq!(actual, admitted_pcm);
    }
    if approved {
        let refused = control
            .fresh_runtime_receipt(submitted)
            .ok_or("refused receipt")?;
        let consent = control
            .offer_device_insert_consent(&refused)
            .ok_or("device cut consent")?;
        control.reclaim();
        let mut receipt = None;
        let installed = control
            .publish_confirmed_device_insert_with_receipt_on_control(
                Playback::try_from_prepared_with_session(successor)?,
                consent,
                |value| receipt = Some(value.clone()),
                |_| Ok::<_, ()>(()),
            )
            .map_err(|_| "approved publication")?;
        let receipt = receipt.ok_or("approved receipt")?;
        let position = control.status.position.load(Ordering::Acquire);
        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected[..FADE])?;
        if let Some(admitted) = admitted_only.as_mut() {
            admitted.render(&mut admitted_pcm[..FADE])?;
            assert_eq!(admitted_pcm[..FADE], expected[..FADE]);
        }
        assert_eq!(
            actual[0],
            expected[0].map(|sample| sample * (127.0 / 128.0))
        );
        assert!(actual[(FADE - 1)..].iter().all(|&frame| frame == [0.0; 2]));
        assert_eq!(receipt.state(), PublicationReceiptState::Applying);
        assert_eq!(control.status.recording_source(), Some(active));
        assert_eq!(
            control.status.position.load(Ordering::Acquire),
            position + FADE as u64
        );

        measured_block(&mut callback, &mut actual);
        oracle.render(&mut expected)?;
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                context,
                audio_revision: installed,
                ..active
            })
        );
        assert_eq!(
            control.status.position.load(Ordering::Acquire),
            position + (FADE + actual.len()) as u64
        );
        if let Some(admitted) = admitted_only.as_mut() {
            admitted.render(&mut admitted_pcm)?;
            assert_ne!(
                admitted_pcm, expected,
                "pending hit must reach the old buses"
            );
            assert!(
                admitted_pcm[FADE..]
                    .iter()
                    .any(|frame| frame[1].abs() > 1e-7)
            );
            assert_eq!(actual[FADE..], admitted_pcm[FADE..]);
        } else {
            assert!(expected[FADE..].iter().any(|frame| frame[1].abs() > 1e-7));
            for (actual, expected) in actual[FADE..].iter().zip(&expected[FADE..]) {
                assert_eq!(actual[1].to_bits(), expected[1].to_bits());
            }
        }
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum NativeInsertLocation {
    Return,
    Track,
    Group,
    Main,
}

#[test]
fn return_delay_append_keeps_reverb_history_and_exact_publication_without_allocations()
-> Result<(), Box<dyn Error>> {
    native_insert_case(NativeInsertLocation::Return, false)
}

#[test]
fn track_group_and_main_delay_append_keeps_reverb_history_and_exact_publication_without_allocations()
-> Result<(), Box<dyn Error>> {
    for location in [
        NativeInsertLocation::Track,
        NativeInsertLocation::Group,
        NativeInsertLocation::Main,
    ] {
        native_insert_case(location, false)?;
    }
    Ok(())
}

#[test]
fn native_mid_insert_keeps_both_effect_histories_and_exact_callback_receipt()
-> Result<(), Box<dyn Error>> {
    for location in [
        NativeInsertLocation::Track,
        NativeInsertLocation::Group,
        NativeInsertLocation::Return,
        NativeInsertLocation::Main,
    ] {
        native_insert_case(location, true)?;
    }
    Ok(())
}

fn native_insert_case(location: NativeInsertLocation, middle: bool) -> Result<(), Box<dyn Error>> {
    use hex_audio::prepared::{AppliedAudioSource, PublicationPolicy, PublicationReceiptState};
    use hex_devices::delay::{
        DEFINITION, DRY_WET, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, TIME,
    };
    use hex_engine::OutputCaptureId;
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::TrackSelectionIds;

    const BLOCK: usize = 512;
    const DELAY: usize = 480;
    const RAMP: usize = 240;
    const SUFFIX_DELAY: usize = 960;
    let delay = |milliseconds| {
        let mut state = DEFINITION.default_state()?;
        for parameter in [LEFT_SYNC, FILTER, HIGH_QUALITY] {
            state = DEFINITION.patch_parameter(&state, parameter, ParameterValue::Bool(false))?;
        }
        for (parameter, value) in [(TIME, milliseconds), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
            state = DEFINITION.patch_parameter(&state, parameter, ParameterValue::Float(value))?;
        }
        Ok::<_, hex_lattice_contract::DeviceContractError>(state)
    };
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { track, asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "reverb input".into(),
        path: "memory/reverb-input.wav".into(),
        sample_rate: RATE,
        frames: 12_000,
    })?
    else {
        return Err("expected source".into());
    };
    let owner = match location {
        NativeInsertLocation::Return => {
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
            Some(OutputCaptureId::Return(bus))
        }
        NativeInsertLocation::Track => Some(OutputCaptureId::Track(track)),
        NativeInsertLocation::Group => {
            let Created::Track(group) = editor.execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track])?,
                name: "room group".into(),
            })?
            else {
                return Err("expected group".into());
            };
            Some(OutputCaptureId::Track(group))
        }
        NativeInsertLocation::Main => None,
    };
    let insert = |before, state| match owner {
        Some(OutputCaptureId::Return(return_track)) => Command::InsertReturnDevice {
            return_track,
            before,
            state,
        },
        Some(OutputCaptureId::Track(track)) => Command::InsertDevice {
            track,
            before,
            state,
        },
        None => Command::InsertMainDevice { before, state },
    };
    let reverb = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?,
        hex_devices::reverb::parameter::DRY_WET,
        ParameterValue::Float(100.0),
    )?;
    editor.execute(insert(None, reverb))?;
    let mut source = vec![[0.0; 2]; 12_000];
    source[..BLOCK].fill([0.25, -0.125]);
    let media = BTreeMap::from([(asset, Audio::new(RATE, source)?)]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        PreparedRevision::prepare(project, &media, Arc::clone(&table), None).map(Arc::new)
    };
    let mut prefix = None;
    let before = if middle {
        let mut playback = Playback::try_from_prepared_with_session(prepare(editor.project())?)?;
        playback.apply(PlaybackCommand::Play);
        prefix = Some(playback);
        let Created::Device(suffix) = editor.execute(insert(None, delay(20.0)?))? else {
            return Err("expected downstream device".into());
        };
        Some(suffix)
    } else {
        None
    };
    let original = prepare(editor.project())?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&original))?;
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
            Playback::try_from_prepared_with_session(original)?,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "initial publication")?;
    control.command(PlaybackCommand::Play).map_err(|_| "play")?;
    let mut warm = [[0.0; 2]; BLOCK];
    for _ in 0..12 {
        oracle.render(&mut warm)?;
        let mut actual = [[0.0; 2]; BLOCK];
        if let Some(prefix) = prefix.as_mut() {
            prefix.render(&mut actual)?;
        }
        measured_block(&mut callback, &mut actual);
        assert_eq!(actual, warm);
    }
    let predecessor = control.status.recording_source().ok_or("original source")?;
    assert_eq!(
        (predecessor.context, predecessor.audio_revision),
        (context, initial)
    );
    let cut = control.status.position.load(Ordering::Acquire);

    editor.execute(insert(before, delay(10.0)?))?;
    let mut next = Playback::try_from_prepared_with_session(prepare(editor.project())?)?;
    next.require_continuous_device_insert();
    let context = hex_engine::AudioSourceContext {
        editor_revision: 1,
        ..context
    };
    let fence = control
        .capture_fresh_runtime_fence()
        .ok_or("device edit fence")?;
    let submitted = control
        .publish_with_admission_and_source_on_control(
            next,
            PublicationPolicy::DeviceInsert { predecessor, fence },
            context,
            |_| Ok::<_, ()>(()),
        )
        .map_err(|_| "append publication")?;
    let receipt = control
        .fresh_runtime_receipt(submitted)
        .ok_or("append receipt")?;
    let mut actual = [[0.0; 2]; BLOCK * 4];
    let mut baseline = actual;
    let mut dry = actual;
    for ((actual, baseline), dry) in actual
        .chunks_mut(BLOCK)
        .zip(baseline.chunks_mut(BLOCK))
        .zip(dry.chunks_mut(BLOCK))
    {
        measured_block(&mut callback, actual);
        oracle.render(baseline)?;
        if let Some(prefix) = prefix.as_mut() {
            prefix.render(dry)?;
        } else {
            dry.copy_from_slice(baseline);
        }
    }
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Installed,
        "{location:?} middle={middle}"
    );
    assert_eq!(
        control.status.recording_source(),
        Some(AppliedAudioSource {
            context,
            audio_revision: submitted,
            ..predecessor
        })
    );
    assert_eq!(
        control.status.position.load(Ordering::Acquire),
        cut + actual.len() as u64
    );
    assert!(control.status.playing.load(Ordering::Acquire));
    assert!(dry[..RAMP].iter().flatten().any(|v| v.abs() > 1e-6));
    assert!(actual[DELAY..].iter().flatten().any(|v| v.abs() > 1e-6));
    let mut upstream = [[0.0; 2]; BLOCK * 4];
    for (frame, output) in upstream.iter_mut().enumerate() {
        let mix = (frame + 1).min(RAMP) as f32 / RAMP as f32;
        for lane in 0..2 {
            let wet: f32 = (1..=frame / DELAY)
                .map(|tap| dry[frame - tap * DELAY][lane] * 0.5_f32.powi(tap as i32 - 1))
                .sum();
            let input = dry[frame][lane];
            output[lane] = input + (wet - input) * mix;
        }
    }
    if middle {
        assert!(
            baseline[..SUFFIX_DELAY]
                .iter()
                .flatten()
                .any(|v| v.abs() > 1e-6)
        );
        for (frame, output) in baseline.iter_mut().enumerate() {
            for lane in 0..2 {
                for tap in 1..=frame / SUFFIX_DELAY {
                    let source = frame - tap * SUFFIX_DELAY;
                    output[lane] +=
                        (upstream[source][lane] - dry[source][lane]) * 0.5_f32.powi(tap as i32 - 1);
                }
            }
        }
    } else {
        baseline = upstream;
    }
    for (frame, (actual, expected)) in actual.iter().zip(&baseline).enumerate() {
        for lane in 0..2 {
            assert!(
                (actual[lane] - expected[lane]).abs() < 2e-6,
                "location={location:?} middle={middle} frame={frame} lane={lane}",
            );
        }
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut warm);
    control.reclaim();
    Ok(())
}
