//! Pure calculations and admission for selected-clip fade controls.
use hex_project::ClipFade;

const FADE_STEP_MILLISECONDS: u64 = 5;

pub(super) fn fade_step_frames(sample_rate: u32) -> u64 {
    ((u64::from(sample_rate) * FADE_STEP_MILLISECONDS + 500) / 1_000).max(1)
}

pub(super) fn fade_ms(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    ((u128::from(frames) * 1_000 + u128::from(sample_rate) / 2) / u128::from(sample_rate)) as u64
}

pub(super) fn fade_edit_admitted(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> bool {
    !opening_project && !close_prompt_open && worker_requests_pending == 0 && pending_requests == 0
}

pub(super) fn fade_is_inherited(fade: ClipFade, duration: u64) -> bool {
    fade.offset() != 0 || fade.span() != duration
}

pub(super) fn compact_fade_time(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds}ms")
    } else if milliseconds < 10_000 {
        let tenths = milliseconds.saturating_add(50) / 100;
        format!("{}.{:01}s", tenths / 10, tenths % 10)
    } else if milliseconds < 1_000_000 {
        format!("{}s", milliseconds / 1_000)
    } else {
        let tenths = milliseconds.saturating_add(50_000) / 100_000;
        format!("{}.{:01}ks", tenths / 10, tenths % 10)
    }
}

pub(super) fn fade_edit_block_reason(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> Option<&'static str> {
    if opening_project {
        Some("fade edit waits for project loading")
    } else if close_prompt_open {
        Some("fade edit waits for the close prompt")
    } else if worker_requests_pending != 0 || pending_requests != 0 {
        Some("fade edit waits for pending project work")
    } else {
        None
    }
}

pub(super) fn adjusted_fades(
    current: u64,
    opposite: u64,
    duration: u64,
    step: u64,
    increase: bool,
) -> Option<(u64, u64)> {
    if duration == 0 {
        return None;
    }
    let next = if increase {
        current.saturating_add(step).min(duration)
    } else {
        current.saturating_sub(step).min(duration)
    };
    Some((next, opposite.min(duration)))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{
        adjusted_fades, compact_fade_time, fade_edit_admitted, fade_edit_block_reason,
        fade_is_inherited, fade_ms, fade_step_frames,
    };

    #[test]
    fn fade_step_and_readouts_use_project_sample_rate() {
        assert_eq!(fade_step_frames(48_000), 240);
        assert_eq!(fade_step_frames(44_100), 221);
        assert_eq!(fade_step_frames(1), 1);
        assert_eq!(fade_ms(240, 48_000), 5);
        assert_eq!(fade_ms(u64::MAX, 0), 0);
    }

    #[test]
    fn fade_edit_rejects_open_close_and_queued_or_inflight_work() {
        assert!(fade_edit_admitted(false, false, 0, 0));
        assert!(!fade_edit_admitted(true, false, 0, 0));
        assert!(!fade_edit_admitted(false, true, 0, 0));
        assert!(!fade_edit_admitted(false, false, 1, 0));
        assert!(!fade_edit_admitted(false, false, 0, 1));
        assert_eq!(
            fade_edit_block_reason(false, false, 1, 0),
            Some("fade edit waits for pending project work")
        );
    }

    #[test]
    fn readouts_stay_compact_with_full_precision_available_elsewhere() {
        assert_eq!(compact_fade_time(100), "100ms");
        assert_eq!(compact_fade_time(1_250), "1.3s");
        assert_eq!(compact_fade_time(12_000), "12s");
        assert_eq!(compact_fade_time(1_250_000), "1.3ks");
    }

    #[test]
    fn fade_adjustments_clamp_both_lengths_to_the_local_clip() {
        assert_eq!(
            adjusted_fades(900, 800, 1_000, 240, true),
            Some((1_000, 800))
        );
        assert_eq!(
            adjusted_fades(1_500, 1_200, 1_000, 240, false),
            Some((1_000, 1_000))
        );
        assert_eq!(adjusted_fades(0, 200, 1_000, 240, false), Some((0, 200)));
    }

    #[test]
    fn split_envelope_domain_is_marked_in_the_readout() {
        let inherited = hex_project::ClipFade::new(1_000, 400, 600, 500).unwrap();
        let reanchored = hex_project::ClipFade::new(600, 0, 600, 500).unwrap();
        assert!(fade_is_inherited(inherited, 600));
        assert!(!fade_is_inherited(reanchored, 600));
    }
}
