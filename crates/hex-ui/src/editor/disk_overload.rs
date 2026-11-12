//! Disk-overload indicator state for the editor status bar.
//!
//! Display-only GPUI state derived from `hex_audio::prepared::Status::
//! stream_starved_callbacks`, a monotonic per-output count the callback
//! publishes when streamed playback is held silent mid-play because disk pages
//! arrived late. The count is read on the existing control-side status poll.
//!
//! Decay rule: the indicator lights when the observed count rises and stays lit
//! for [`HOLD`] after the latest rise, then clears. A different output or a
//! reset count clears the previous indication and establishes a new baseline.

use std::time::{Duration, Instant};

/// How long the indicator stays lit after the latest starvation rise.
pub(super) const HOLD: Duration = Duration::from_secs(3);

#[derive(Debug, Default)]
pub(super) struct DiskOverload {
    baseline: Option<(u64, u64)>,
    lit_until: Option<Instant>,
    /// Lit state returned by the latest observation, so expiry redraws once.
    shown: bool,
}

impl DiskOverload {
    /// Observe the applied output identity and its cumulative starvation count.
    /// Returns whether the displayed state (lit flag or lit count) changed.
    pub(super) fn observe(&mut self, starved: Option<(u64, u64)>, now: Instant) -> bool {
        let previous = self.baseline;
        self.baseline = starved;
        match (previous, starved) {
            (Some((prior_output, prior)), Some((output, current)))
                if output == prior_output && current >= prior =>
            {
                if current > prior {
                    self.lit_until = now.checked_add(HOLD);
                }
            }
            _ => self.lit_until = None,
        }
        if !self.lit(now) {
            self.lit_until = None;
        }
        let lit = self.lit(now);
        let changed = self.shown != lit || (lit && previous != starved);
        self.shown = lit;
        changed
    }

    pub(super) fn lit(&self, now: Instant) -> bool {
        self.lit_until.is_some_and(|until| now < until)
    }

    /// Lit state as of the latest poll; rendering reads this, not the clock,
    /// so paint always matches the redraw decision.
    pub(super) fn shown(&self) -> bool {
        self.shown
    }

    /// Callbacks starved on the current output, for the tooltip.
    pub(super) fn count(&self) -> u64 {
        self.baseline.map_or(0, |(_, count)| count)
    }
}

/// Lowercase tooltip text; rendered through `theme::text`.
pub(super) fn description(count: u64) -> String {
    format!(
        "disk overload · streamed audio was held silent in {count} callbacks on this output · \
         clears {}s after the last",
        HOLD.as_secs()
    )
}
