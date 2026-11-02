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
