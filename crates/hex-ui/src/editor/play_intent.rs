//! UI-side transport intent for rapid commands before callback feedback.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IntentToken(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TokenExhausted;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionError {
    QueueFull,
    TokenExhausted,
}

#[derive(Clone, Copy, Debug)]
struct Intent {
    token: IntentToken,
    desired_playing: bool,
    ticket: Option<u64>,
}

#[derive(Debug)]
pub(super) struct Tracker {
    next_token: u64,
    latest: Option<Intent>,
    priority_stop: bool,
}

pub(super) fn has_ui_queue_capacity(pending: usize, capacity: usize) -> bool {
    pending < capacity
}

pub(super) fn prepare_admission(
    tracker: &mut Tracker,
    desired_playing: Option<bool>,
    pending: usize,
    capacity: usize,
) -> Result<Option<IntentToken>, AdmissionError> {
    if !has_ui_queue_capacity(pending, capacity) {
        return Err(AdmissionError::QueueFull);
    }
    desired_playing
        .map(|_| {
            tracker
                .reserve_token()
                .map_err(|_| AdmissionError::TokenExhausted)
        })
        .transpose()
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            next_token: 1,
            latest: None,
            priority_stop: false,
        }
    }
}

impl Tracker {
    /// Reserve a unique token before admission. Call `admit` only after the
    /// control or bounded UI queue accepts the corresponding action.
    pub(super) fn reserve_token(&mut self) -> Result<IntentToken, TokenExhausted> {
        let token = IntentToken(self.next_token);
        self.next_token = self.next_token.checked_add(1).ok_or(TokenExhausted)?;
        Ok(token)
    }

    pub(super) fn admit(&mut self, token: IntentToken, desired_playing: bool) {
        self.latest = Some(Intent {
            token,
            desired_playing,
            ticket: None,
        });
        self.priority_stop = false;
    }

    pub(super) fn attach_ticket(&mut self, token: IntentToken, ticket: u64) {
        if let Some(intent) = self.latest.as_mut()
            && intent.token == token
        {
            intent.ticket = Some(ticket);
        }
    }

    pub(super) fn clear_if_current(&mut self, token: IntentToken) {
        if self.latest.is_some_and(|intent| intent.token == token) {
            self.latest = None;
        }
    }

    pub(super) fn desired_playing(&self, actual_playing: bool) -> bool {
        if self.priority_stop {
            return false;
        }
        self.latest
            .map_or(actual_playing, |intent| intent.desired_playing)
    }

    /// Priority Stop never needs a UI token or bounded-queue capacity.
    pub(super) fn priority_stop(&mut self) {
        self.latest = None;
        self.priority_stop = true;
    }

    pub(super) fn clear(&mut self) {
        self.latest = None;
        self.priority_stop = false;
    }

    /// Resolve only the latest intent. An earlier command's consumed ticket
    /// cannot erase a newer unsent Play/Pause intent.
    pub(super) fn reconcile(
        &mut self,
        consumed_high_water: u64,
        actual_playing: bool,
        rejected_ticket: Option<u64>,
    ) {
        if self.priority_stop && !actual_playing {
            self.priority_stop = false;
        }
        if self.latest.is_some_and(|intent| {
            intent.ticket.is_some_and(|ticket| {
                rejected_ticket == Some(ticket) || consumed_high_water >= ticket
            })
        }) {
            self.latest = None;
        }
    }

    /// Resolve only the exact admitted action; later tickets cannot prove that
    /// an earlier streamed action has terminated.
    pub(super) fn reconcile_receipt(&mut self, ticket: u64, actual_playing: bool) {
        self.reconcile_actual(actual_playing);
        if self
            .latest
            .is_some_and(|intent| intent.ticket == Some(ticket))
        {
            self.latest = None;
        }
    }

    pub(super) fn reconcile_actual(&mut self, actual_playing: bool) {
        if self.priority_stop && !actual_playing {
            self.priority_stop = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionError, IntentToken, TokenExhausted, Tracker, prepare_admission};

    #[test]
    fn launch_then_space_tracks_pause_then_play() {
        let mut tracker = Tracker::default();
        let launch = tracker.reserve_token().expect("launch token");
        tracker.admit(launch, true);
        assert!(tracker.desired_playing(false));

        let pause = tracker.reserve_token().expect("pause token");
        tracker.admit(pause, false);
        assert!(!tracker.desired_playing(false));

        let play = tracker.reserve_token().expect("play token");
        tracker.admit(play, true);
        assert!(tracker.desired_playing(false));
    }

    #[test]
    fn direct_play_pause_order_is_latest_intent_wins() {
        let mut tracker = Tracker::default();
        let play = tracker.reserve_token().expect("play token");
        tracker.admit(play, true);
        let pause = tracker.reserve_token().expect("pause token");
        tracker.admit(pause, false);
        assert!(!tracker.desired_playing(false));
    }

    #[test]
    fn stale_launch_clear_cannot_erase_a_newer_pause() {
        let mut tracker = Tracker::default();
        let launch = tracker.reserve_token().expect("launch token");
        tracker.admit(launch, true);
        let pause = tracker.reserve_token().expect("pause token");
        tracker.admit(pause, false);
        tracker.clear_if_current(launch);
        assert!(!tracker.desired_playing(true));
    }

    #[test]
    fn earlier_ticket_does_not_resolve_a_newer_unsent_intent() {
        let mut tracker = Tracker::default();
        let launch = tracker.reserve_token().expect("launch token");
        tracker.admit(launch, true);
        tracker.attach_ticket(launch, 8);
        let pause = tracker.reserve_token().expect("pause token");
        tracker.admit(pause, false);
        tracker.attach_ticket(launch, 8);
        tracker.reconcile(8, true, None);
        assert!(!tracker.desired_playing(true));
    }

    #[test]
    fn later_receipt_does_not_resolve_an_earlier_pending_intent() {
        let mut tracker = Tracker::default();
        let token = tracker.reserve_token().unwrap();
        tracker.admit(token, true);
        tracker.attach_ticket(token, 4);
        tracker.reconcile_receipt(5, false);
        assert!(tracker.desired_playing(false));
        tracker.reconcile_receipt(4, false);
        assert!(!tracker.desired_playing(false));
    }

    #[test]
    fn consumed_or_failed_latest_ticket_resolves_to_actual_state() {
        let mut tracker = Tracker::default();
        let launch = tracker.reserve_token().expect("launch token");
        tracker.admit(launch, true);
        tracker.attach_ticket(launch, 11);
        tracker.reconcile(0, false, Some(11));
        assert!(!tracker.desired_playing(false));

        let next = tracker.reserve_token().expect("next token");
        tracker.admit(next, true);
        assert!(tracker.desired_playing(false));
    }

    #[test]
    fn full_ui_queue_keeps_existing_intent_unchanged() {
        let mut tracker = Tracker::default();
        let play = prepare_admission(&mut tracker, Some(true), 0, 32)
            .expect("play admission")
            .expect("play token");
        tracker.admit(play, true);
        assert_eq!(
            prepare_admission(&mut tracker, Some(false), 32, 32),
            Err(AdmissionError::QueueFull)
        );
        assert!(tracker.desired_playing(false));
    }

    #[test]
    fn priority_stop_and_lifecycle_clear_do_not_depend_on_tokens() {
        let mut tracker = Tracker::default();
        let play = tracker.reserve_token().expect("play token");
        tracker.admit(play, true);
        tracker.priority_stop();
        assert!(!tracker.desired_playing(true));
        tracker.reconcile(0, false, None);
        assert!(!tracker.desired_playing(false));

        let next = tracker.reserve_token().expect("next token");
        tracker.admit(next, true);
        tracker.clear();
        assert!(!tracker.desired_playing(false));
    }

    #[test]
    fn token_exhaustion_is_checked_without_aliasing() {
        let mut tracker = Tracker {
            next_token: u64::MAX,
            latest: None,
            priority_stop: false,
        };
        assert_eq!(tracker.reserve_token(), Err(TokenExhausted));
        assert_eq!(tracker.reserve_token(), Err(TokenExhausted));
        assert!(tracker.latest.is_none());
        assert_eq!(tracker.next_token, u64::MAX);
        tracker.priority_stop();
        assert!(!tracker.desired_playing(true));
        assert_ne!(IntentToken(1), IntentToken(u64::MAX));
    }
}
