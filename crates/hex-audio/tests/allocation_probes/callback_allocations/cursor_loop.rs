//! Real command consumption and looping share the allocation probe boundary.
use super::*;

#[test]
fn absolute_cursor_before_and_after_loop_is_callback_allocation_free() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "cursor".into(),
        path: "cursor.wav".into(),
        sample_rate: RATE,
        frames: 16,
    })?
    else {
        return Err("expected audio import".into());
    };
    editor.execute(Command::SetLoopRegion {
        start: 4,
        end: 8,
        enabled: true,
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::from([(
            asset,
            Audio::new(RATE, (1..=16).map(|n| [n as f32; 2]).collect())?,
        )]),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&graph))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "cursor publication failed")?;
    control
        .command(PlaybackCommand::PlayFrom(12))
        .map_err(|_| "post-loop start failed")?;
    let mut output = [[0.0; 2]; 6];
    measured_block(&mut callback, &mut output);
    let observation = control
        .status
        .transport_observation()
        .ok_or("missing transport observation")?;
    assert_eq!(observation.arrangement_frame, 18);
    assert!(observation.playing);
    assert_eq!(
        observation
            .loop_state
            .map(|state| (state.pass, state.active_region)),
        Some((0, None))
    );
    assert_eq!(
        output,
        [
            [13.0; 2], [14.0; 2], [15.0; 2], [16.0; 2], [0.0; 2], [0.0; 2]
        ]
    );
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "stop failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 6]);
    let observation = control
        .status
        .transport_observation()
        .ok_or("missing stopped observation")?;
    // A declicked Stop may still be finishing its fade in this short block.
    assert_eq!(
        observation.arrangement_frame,
        control.status.position.load(Ordering::Relaxed)
    );
    control
        .command(PlaybackCommand::PlayFrom(2))
        .map_err(|_| "pre-loop start failed")?;
    measured_block(&mut callback, &mut []);
    let before_loop = control.status.transport_observation().unwrap();
    assert_eq!(before_loop.arrangement_frame, 2);
    assert_eq!(
        before_loop
            .loop_state
            .map(|state| (state.pass, state.active_region)),
        Some((0, Some((4, 8))))
    );
    measured_block(&mut callback, &mut output);
    let region = (4, 8);
    assert_eq!(
        control
            .status
            .transport_observation()
            .unwrap()
            .loop_state
            .map(|state| (state.pass, state.active_region)),
        Some((1, Some(region)))
    );
    // The loop's last sample fades to zero; its next attack stays unattenuated.
    assert_eq!(
        output,
        [[3.0; 2], [4.0; 2], [5.0; 2], [6.0; 2], [7.0; 2], [0.0; 2]]
    );
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control
            .status
            .transport_observation()
            .unwrap()
            .loop_state
            .map(|state| (state.pass, state.active_region)),
        Some((2, Some(region)))
    );
    assert_eq!(
        output,
        [[5.0; 2], [6.0; 2], [7.0; 2], [0.0; 2], [5.0; 2], [6.0; 2]]
    );
    let replacement = control
        .publish_with_policy(
            Playback::try_from_prepared_with_session(graph)?,
            prepared::PublicationPolicy::InstallFreshRuntime,
        )
        .map_err(|_| "replacement publication failed")?;
    measured_block(&mut callback, &mut output);
    let observation = control.status.transport_observation().unwrap();
    assert_eq!(observation.audio_revision, replacement);
    assert_eq!(
        observation
            .loop_state
            .map(|state| (state.pass, state.active_region)),
        Some((0, Some(region)))
    );
    control.request_shutdown();
    assert_eq!(control.status.transport_observation(), None);
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn event_fade_pcm_publication_seek_stop_and_shutdown_allocate_and_free_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    use hex_project::{AudioEventEdit, AudioEventFade, ClipFade};
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { asset, clip, .. } = editor.execute(Command::ImportAudioTrack {
        name: "event fade".into(),
        path: "original.wav".into(),
        sample_rate: RATE,
        frames: 16,
    })?
    else {
        return Err("expected audio import".into());
    };
    editor.execute(Command::EditAudioEvents {
        clip,
        operation: AudioEventEdit::SetFade {
            event: 1,
            fade: Some(AudioEventFade::new(0, ClipFade::new(16, 0, 4, 4)?)?),
        },
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::from([(asset, Audio::new(RATE, vec![[1.0; 2]; 16])?)]),
        Arc::new(SineTable::new()),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(graph)?)
        .map_err(|_| "fade publication failed")?;
    control
        .command(PlaybackCommand::PlayFrom(0))
        .map_err(|_| "fade start failed")?;
    let mut output = [[0.0; 2]; 8];
    measured_block(&mut callback, &mut output);
    for (frame, gain) in output
        .iter()
        .zip([0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0, 1.0])
    {
        assert_eq!(*frame, [gain; 2]);
    }
    control
        .command(PlaybackCommand::PlayFrom(12))
        .map_err(|_| "fade seek failed")?;
    // The queued seek first fades the old position over 128 frames. Only
    // eight source frames remain there; their clip fade and transport fade
    // both apply before the target begins its independent fade-in.
    let mut transition = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut transition);
    for (index, frame) in transition.iter().enumerate() {
        let clip_gain = match index {
            0..=4 => 1.0,
            5 => 2.0 / 3.0,
            6 => 1.0 / 3.0,
            _ => 0.0,
        };
        let gain = clip_gain * (127 - index) as f32 / 128.0;
        assert_eq!(*frame, [gain; 2], "old frame {index}");
    }
    measured_block(&mut callback, &mut output);
    for (index, frame) in output.iter().enumerate() {
        let clip_gain = match index {
            0 => 1.0,
            1 => 2.0 / 3.0,
            2 => 1.0 / 3.0,
            _ => 0.0,
        };
        assert_eq!(
            *frame,
            [clip_gain * (index + 1) as f32 / 128.0; 2],
            "target frame {index}"
        );
    }
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "fade stop failed")?;
    measured_block(&mut callback, &mut transition);
    measured_block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 8]);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn selection_crossfade_publication_seek_stop_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported {
        track,
        asset,
        clip: left,
    } = editor.execute(Command::ImportAudioTrack {
        name: "crossfade".into(),
        path: "original-crossfade.wav".into(),
        sample_rate: RATE,
        frames: 16,
    })?
    else {
        return Err("expected audio import".into());
    };
    editor.execute(Command::SetClipLooping {
        clip: left,
        looping: true,
    })?;
    editor.execute(Command::TrimClipEdge {
        clip: left,
        edge: hex_project::TrimEdge::Right,
        frame: 8,
    })?;
    let Created::Clip(right) = editor.execute(Command::CopyClipTo {
        clip: left,
        track,
        start: 8,
    })?
    else {
        return Err("expected audio copy".into());
    };
    editor.execute(Command::SetClipSourcePhase {
        clip: right,
        phase: 5,
    })?;
    for clip in [left, right] {
        editor.execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })?;
    }
    editor.execute(Command::CreateArrangementFades {
        range: hex_project::TimeRange::new(vec![track], 6, 10)?,
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::from([(
            asset,
            Audio::new(RATE, (1..=16).map(|n| [n as f32 / 16.0; 2]).collect())?,
        )]),
        Arc::new(SineTable::new()),
        None,
    )?);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(Playback::try_from_prepared_with_session(graph)?)
        .map_err(|_| "crossfade publication")?;
    control
        .command(PlaybackCommand::PlayFrom(4))
        .map_err(|_| "crossfade seek")?;
    let mut output = [[0.0; 2]; 12];
    measured_block(&mut callback, &mut output);
    for (offset, frame) in output.iter().enumerate() {
        let at = 4 + offset;
        let expected = if at < 6 {
            (at + 1) as f32 / 16.0
        } else if at < 10 {
            let progress = (at - 6) as f64 / 3.0;
            let incoming_gain = progress * progress * (3.0 - 2.0 * progress);
            (at + 1) as f32 / 16.0 * (1.0 - incoming_gain) as f32
                + (at - 2) as f32 / 16.0 * incoming_gain as f32
        } else {
            (at - 2) as f32 / 16.0
        };
        for sample in frame {
            assert!((*sample - expected).abs() < 1e-7, "frame {at}");
        }
    }
    control.reclaim();
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "crossfade stop")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 12]);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn silent_left_prefix_publication_keeps_pcm_and_allocates_or_frees_nothing()
-> Result<(), Box<dyn Error>> {
    for (cents, once, musical) in [
        (0, false, false),
        (137, false, false),
        (137, true, false),
        (137, false, true),
    ] {
        let mut editor = Editor::new(Project::new(RATE)?, 16)?;
        let Created::Imported { track, asset, clip } =
            editor.execute(Command::ImportAudioTrack {
                name: "prefix".into(),
                path: "prefix.wav".into(),
                sample_rate: RATE,
                frames: 64,
            })?
        else {
            return Err("expected import".into());
        };
        editor.execute(Command::MoveClip {
            clip,
            track,
            start: 32,
        })?;
        editor.execute(Command::SetClipFadeEnvelope {
            clip,
            fade: hex_project::ClipFade::new(64, 0, 4, 4)?,
        })?;
        if cents != 0 {
            editor.execute(Command::SetAudioClipPitch { clip, cents })?;
        }
        if musical {
            editor.execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })?;
        }
        if once {
            editor.execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: 0,
                    end: 48,
                    looping: false,
                },
            })?;
        }
        let media = BTreeMap::from([(asset, Audio::new(RATE, vec![[0.5; 2]; 64])?)]);
        let sine = Arc::new(SineTable::new());
        let original = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::clone(&sine),
            None,
        )?);
        let mut reference = Playback::try_from_prepared_with_session(Arc::clone(&original))?;
        reference.apply(PlaybackCommand::PlayFrom(16));
        let mut graphs = vec![Arc::clone(&original)];
        for frame in [16, 8] {
            editor.execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame,
            })?;
            graphs.push(Arc::new(PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::clone(&sine),
                None,
            )?));
        }
        let (mut control, mut callback) = prepared::channel(RATE)?;
        control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(
                &original,
            ))?)
            .map_err(|_| "publication failed")?;
        control
            .command(PlaybackCommand::PlayFrom(16))
            .map_err(|_| "play failed")?;
        for block in 0..12 {
            if block == 1 || block == 4 {
                let index = if block == 1 { 1 } else { 2 };
                control
                    .publish(Playback::try_from_prepared_with_session(Arc::clone(
                        &graphs[index],
                    ))?)
                    .map_err(|_| "prefix publication failed")?;
            }
            let mut expected = [[0.0; 2]; 8];
            let mut actual = [[0.0; 2]; 8];
            reference.render(&mut expected)?;
            measured_block(&mut callback, &mut actual);
            compare_output(&actual, &expected);
            control.reclaim();
        }
        control.request_shutdown();
        measured_shutdown(&mut callback, &mut [[0.0; 2]; 8]);
        control.reclaim();
    }
    Ok(())
}

#[test]
fn group_output_mute_publication_preserves_clock_and_allocates_nothing_in_callback()
-> Result<(), Box<dyn Error>> {
    use hex_project::{GroupOutputMuteRange, TrackSelectionIds};
    const FRAMES: usize = 640;
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    for (name, frame) in [("group", [0.25, 0.125]), ("unrelated", [0.125, 0.0625])] {
        let Created::Imported { asset, track, .. } = editor.execute(Command::ImportAudioTrack {
            name: name.into(),
            path: format!("{name}.wav").into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })?
        else {
            return Err("expected audio import".into());
        };
        media.insert(asset, Audio::new(RATE, vec![frame; FRAMES])?);
        tracks.push(track);
    }
    let Created::Track(group) = editor.execute(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&tracks[..1])?,
        name: "selected".into(),
    })?
    else {
        return Err("expected group".into());
    };
    let source = editor.project();
    let mut gated = source.clone();
    let interval = GroupOutputMuteRange::new(83, 117)?;
    gated.add_group_output_mute(group, interval)?;
    let sine = Arc::new(SineTable::new());
    let plain = Arc::new(PreparedRevision::prepare(
        source,
        &media,
        Arc::clone(&sine),
        None,
    )?);
    let gated = Arc::new(PreparedRevision::prepare(&gated, &media, sine, None)?);
    for block in [1, 7, 64, 512] {
        let (mut control, mut callback) = prepared::channel(RATE)?;
        control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(
                &plain,
            ))?)
            .map_err(|_| "plain publication failed")?;
        control
            .command(PlaybackCommand::PlayFrom(0))
            .map_err(|_| "start failed")?;
        let mut warm = [[0.0; 2]; 16];
        measured_block(&mut callback, &mut warm);
        assert_eq!(warm, [[0.375, 0.1875]; 16]);
        control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(
                &gated,
            ))?)
            .map_err(|_| "mute publication failed")?;
        let mut output = [[0.0; 2]; FRAMES - 16];
        for chunk in output.chunks_mut(block) {
            measured_block(&mut callback, chunk);
        }
        for (offset, frame) in output.into_iter().enumerate() {
            let expected = if interval.contains((16 + offset) as u64) {
                [0.125, 0.0625]
            } else {
                [0.375, 0.1875]
            };
            assert_eq!(frame, expected, "arrangement frame {}", 16 + offset);
        }
        control
            .command(PlaybackCommand::Stop)
            .map_err(|_| "stop failed")?;
        measured_block(&mut callback, &mut warm);
        assert_eq!(warm, [[0.0; 2]; 16]);
        control.request_shutdown();
        measured_shutdown(&mut callback, &mut warm);
        control.reclaim();
    }
    Ok(())
}
