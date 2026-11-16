//! Real prepared-audio control coverage for recovery state.
use super::recovery_state::RecoveryState;

#[test]
fn real_new_control_restores_cursor_without_inheriting_old_tickets_or_play() {
    use hex_audio::prepared::channel;
    use hex_engine::{
        Project,
        playback::{Playback, PlaybackCommand},
    };
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    let (mut old_control, mut old_callback) = channel(48_000).expect("old control");
    old_control
        .publish(Playback::new(
            Project::new(48_000, vec![]).expect("old graph"),
        ))
        .map_err(|error| error.reason)
        .expect("old publication");
    for frame in [32, 64, 96] {
        old_control
            .command_with_ticket(PlaybackCommand::Seek(frame))
            .expect("old seek");
    }
    old_callback.begin_block();
    assert_eq!(old_control.status.consumed_command.load(Acquire), 3);

    let (mut control, mut callback) = channel(48_000).expect("replacement control");
    let mut state = RecoveryState::default();
    let generation = state.begin(96).expect("recovery");
    state.opened(generation);
    state.restored(generation);
    let graph = Project::new(48_000, vec![]).expect("empty graph");
    let revision = control
        .publish(Playback::new(graph))
        .map_err(|error| error.reason)
        .expect("graph publication");
    assert!(!state.complete(false, 3, 96, false));
    callback.begin_block();
    let current = control.status.applied_revision.load(Relaxed) == revision;
    let position = state
        .seek_needed(current)
        .expect("seek after graph applied");
    let ticket = control
        .command_with_ticket(PlaybackCommand::Seek(position))
        .expect("new seek");
    assert_eq!(ticket, 1);
    state.seeking(ticket);
    assert!(!state.complete(
        current,
        control.status.consumed_command.load(Acquire),
        control.status.position.load(Relaxed),
        false
    ));
    callback.begin_block();
    let mut output = [[1.0; 2]; 4];
    callback.render(&mut output).expect("paused callback");
    assert_eq!(output, [[0.0; 2]; 4]);
    assert!(state.complete(
        current,
        control.status.consumed_command.load(Acquire),
        control.status.position.load(Relaxed),
        control.status.playing.load(Relaxed)
    ));
    assert_eq!(control.status.position.load(Relaxed), 96);
    assert!(!control.status.playing.load(Relaxed));
}
