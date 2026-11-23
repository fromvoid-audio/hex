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

#[test]
fn prepared_recovery_retry_and_latest_cursor_keep_exact_receipts() {
    let mut state = RecoveryState::default();
    let generation = state.begin(129).unwrap();
    assert!(state.opened(generation));
    assert!(state.restored(generation));
    assert!(state.prepared_seeking(generation, 1));
    assert!(!state.retry_prepared_seek(generation + 1, 1));
    assert!(!state.retry_prepared_seek(generation, 2));
    assert!(state.retry_prepared_seek(generation, 1));
    assert_eq!(state.seek_needed(true), Some(129));
    assert!(state.prepared_seeking(generation, 2));
    assert!(!state.retry_prepared_seek(generation, 2));
    state.prepared_published(2, 7);
    state.retarget(193);
    assert!(!state.complete_prepared(true, Some((1, 7)), 7, 129, false));
    assert_eq!(state.prepared_request(), Some(2));
    assert!(!state.complete_prepared(true, Some((2, 7)), 7, 129, false));
    assert_eq!(state.seek_needed(true), Some(193));
    assert!(state.prepared_seeking(generation, 3));
    state.prepared_published(3, 8);
    assert!(!state.complete_prepared(true, Some((2, 7)), 8, 193, false));
    assert!(state.complete_prepared(true, Some((3, 8)), 8, 193, false));
    assert_eq!(state.generation(), None);
}
