use super::{SessionTicketOutcome, reconcile_session_receipt};
use hex_engine::PlaybackAction;
use hex_engine::playback::Playback;

#[test]
fn earlier_session_rejection_is_not_reported_as_consumed() {
    let (mut control, mut callback) = hex_audio::prepared::channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
    control.publish(playback).unwrap();
    callback.begin_block();
    let first = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Stop)
        .unwrap();
    let second = control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    callback.begin_block();
    assert_eq!(
        control
            .status
            .consumed_command
            .load(std::sync::atomic::Ordering::Acquire),
        second
    );
    assert_eq!(
        reconcile_session_receipt(Some((first, 7)), 7, reader.try_take().unwrap()),
        (None, Some(SessionTicketOutcome::Rejected)),
        "a later rejection cannot turn the earlier failed action into consumed feedback"
    );
    assert_eq!(reader.try_take().unwrap().ticket, second);
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}
