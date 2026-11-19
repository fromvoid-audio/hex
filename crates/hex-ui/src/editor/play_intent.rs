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
    // The standalone unit target exercises exhaustion without exposing a runtime setter.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn starting_at(next_token: u64) -> Self {
        Self {
            next_token,
            ..Self::default()
        }
    }

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
