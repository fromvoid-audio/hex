//! The declicked Pause/Stop fade, its split render and resume ramp are
//! measured on the rendering thread with construction outside the probe.
use super::*;
use hex_engine::playback::PlaybackAction;

fn measured(playback: &mut Playback, action: PlaybackCommand, output: &mut [Frame]) {
    let mut failed = false;
    let mut observe = |_: hex_engine::ProjectSignal, _: usize, _: &[Frame]| {};
    let tracks = [];
    let mut tap = hex_engine::ProjectOutputTap::new(&tracks, &mut observe);
    let info = measure(|| {
        failed |= playback
            .try_apply_declicked_action(PlaybackAction::Transport(action))
            .is_err();
        failed |= playback
            .render_with_signal_observers(output, None, tap.as_mut())
            .is_err();
    });
    assert!(!failed, "declicked {action:?} failed");
    assert_eq!(info.count_total, 0, "{action:?} allocated");
    assert_eq!(info.bytes_total, 0, "{action:?} allocated bytes");
    assert_eq!(info.count_current, 0, "{action:?} changed live allocations");
    assert_eq!(info.bytes_current, 0, "{action:?} changed live bytes");
}

#[test]
fn declicked_pause_resume_and_stop_are_callback_allocation_free() -> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let mut playback = Playback::try_from_prepared_with_session(prepare_fixture(&workload)?)?;
    let mut output = [[0.0; 2]; 300];
    // Warm the ordinary playing path outside the probe.
    playback.apply(PlaybackCommand::Play);
    playback.render(&mut output)?;
    for action in [
        PlaybackCommand::Pause,
        PlaybackCommand::Play,
        PlaybackCommand::Stop,
    ] {
        // Each block crosses its fade end, so it renders as two parts.
        measured(&mut playback, action, &mut output);
        assert_eq!(playback.pending_transport_fade(), None, "{action:?}");
    }
    assert!(!playback.playing());
    Ok(())
}

/// The prepared callback consumes a queued Pause/Stop and settles its
/// deferred host effects at the fade end inside measured blocks (#4151).
#[test]
fn queued_callback_pause_and_stop_fades_are_allocation_free() -> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let playback = Playback::try_from_prepared_with_session(prepare_fixture(&workload)?)?;
    let (mut control, mut callback) = prepared::channel(playback.sample_rate())?;
    control
        .publish(playback)
        .map_err(|_| "active publication failed")?;
    let mut output = [[0.0; 2]; 300];
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play admission failed")?;
    callback.begin_block();
    callback.render(&mut output)?;
    for command in [
        PlaybackCommand::Seek(1000),
        PlaybackCommand::Pause,
        PlaybackCommand::Play,
        PlaybackCommand::Stop,
    ] {
        control
            .command(command)
            .map_err(|_| "command admission failed")?;
        // Each 300-frame block crosses the 128-frame fade end.
        measured_block(&mut callback, &mut output);
        assert_eq!(
            control.status.playing.load(Ordering::Acquire),
            matches!(command, PlaybackCommand::Play | PlaybackCommand::Seek(_)),
            "{command:?}"
        );
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    Ok(())
}

#[test]
fn queued_session_seek_and_play_from_fades_are_allocation_free() -> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let playback = Playback::try_from_prepared_with_session(prepare_fixture(&workload)?)?;
    let (mut control, mut callback) = prepared::channel(playback.sample_rate())?;
    control
        .publish(playback)
        .map_err(|_| "publication failed")?;
    let mut output = [[0.0; 2]; 300];
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "play refused")?;
    callback.begin_block();
    callback.render(&mut output)?;
    for action in [
        PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Seek(1000)),
        PlaybackAction::Transport(PlaybackCommand::PlayFrom(2000)),
    ] {
        control
            .action_with_ticket(action)
            .map_err(|_| "seek admission failed")?;
        measured_block(&mut callback, &mut output);
        assert!(control.status.playing.load(Ordering::Acquire), "{action:?}");
        let target = match action {
            PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Seek(frame))
            | PlaybackAction::Transport(PlaybackCommand::PlayFrom(frame)) => frame,
            _ => return Err("unexpected probe action".into()),
        };
        assert_eq!(
            control.status.position.load(Ordering::Acquire),
            target + 172,
            "{action:?}"
        );
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    Ok(())
}
