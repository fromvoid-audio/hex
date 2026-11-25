//! Pure status-text helpers shared across editor controllers.

pub fn clear_stale_playback_unavailable_error<E: std::ops::Deref<Target = str>>(
    error: &mut Option<E>,
) -> bool {
    let stale = error.as_deref().is_some_and(|message| {
        matches!(
            message,
            "playback source is unavailable · cannot seek safely"
                | "playback source is unavailable · retry playback when audio is ready"
        )
    });
    if stale {
        *error = None;
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::clear_stale_playback_unavailable_error;

    #[test]
    fn clears_only_playback_unavailable_status_messages() {
        let mut seek_msg = Some("playback source is unavailable · cannot seek safely");
        assert!(clear_stale_playback_unavailable_error(&mut seek_msg));
        assert!(seek_msg.is_none());

        let mut retry_msg =
            Some("playback source is unavailable · retry playback when audio is ready");
        assert!(clear_stale_playback_unavailable_error(&mut retry_msg));
        assert!(retry_msg.is_none());

        let mut other = Some("the seek was cancelled");
        assert!(!clear_stale_playback_unavailable_error(&mut other));
        assert_eq!(other, Some("the seek was cancelled"));
    }
}
