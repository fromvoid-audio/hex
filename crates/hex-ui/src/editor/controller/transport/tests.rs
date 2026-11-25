//! Controller transport helper unit tests.
use super::*;

#[test]
fn paused_republish_rejection_matches_only_expected_handoff_reasons() {
    assert!(is_paused_republish_rejection(
        hex_engine::PlaybackHandoffRejection::HeldArrangementGateChanged
    ));
    assert!(is_paused_republish_rejection(
        hex_engine::PlaybackHandoffRejection::SessionRuntime
    ));
    assert!(is_paused_republish_rejection(
        hex_engine::PlaybackHandoffRejection::ForeignTailSourcePaused
    ));
}

#[test]
fn clear_stale_playback_unavailable_error_clears_only_playback_unavailable_messages() {
    let mut stale: Option<String> =
        Some("playback source is unavailable · cannot seek safely".into());
    assert!(clear_stale_playback_unavailable_error(&mut stale));
    assert!(stale.is_none());

    let mut retry: Option<String> =
        Some("playback source is unavailable · retry playback when audio is ready".into());
    assert!(clear_stale_playback_unavailable_error(&mut retry));
    assert!(retry.is_none());

    let mut unrelated: Option<String> = Some("save failed · disk full".into());
    assert!(!clear_stale_playback_unavailable_error(&mut unrelated));
    assert_eq!(unrelated.as_deref(), Some("save failed · disk full"));
}
