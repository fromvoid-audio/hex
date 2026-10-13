//! Musical typing identities and bounded keyboard/release protocol.
//! Queue acceptance is distinct from actual native application. Bridges must
//! deliver cancellation before their input prefix and charge all retained state.

pub const KEYS: [&str; 12] = ["a", "w", "s", "e", "d", "f", "t", "g", "y", "h", "u", "j"];
pub const MAX_VOICES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub generation: u64,
    pub track: u64,
    pub device: u64,
}
impl Owner {
    pub fn valid(self) -> bool {
        self.generation != 0 && self.track != 0 && self.device != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub epoch: u64,
    pub serial: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    Authored {
        track: u64,
        clip: u64,
        note: u64,
        pass: u64,
    },
    Live {
        owner: Owner,
        token: Token,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    On {
        owner: Owner,
        token: Token,
        key: u8,
        pitch: u8,
        velocity: u8,
    },
    Off {
        owner: Owner,
        token: Token,
    },
}
impl Action {
    pub fn identity(self) -> Identity {
        let (owner, token) = match self {
            Self::On { owner, token, .. } | Self::Off { owner, token } => (owner, token),
        };
        Identity::Live { owner, token }
    }
    pub fn token(self) -> Token {
        match self {
            Self::On { token, .. } | Self::Off { token, .. } => token,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutReason {
    Stop,
    Seek,
    Reset,
    Disarm,
    WindowInactive,
    AudioDisconnected,
    ReleaseQueueFull,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub through_epoch: u64,
    pub reason: CutReason,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressState {
    Offered,
    Queued(u64),
    Applied,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    action: Action,
    state: PressState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub active_window: bool,
    pub text_editing: bool,
    pub modal: bool,
    pub modified: bool,
}
impl Focus {
    fn allows_on(self) -> bool {
        self.active_window && !self.text_editing && !self.modal && !self.modified
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Playing,
    Paused,
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Stale,
    Invalid,
    NotPlaying,
    NotAdmitted,
    Capacity,
    IdentityExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    OnApplied,
    OffPending,
    Refused(Refusal),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedResult {
    pub action: Action,
    pub kind: ResultKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyboard {
    owner: Owner,
    epoch: u64,
    serial: u64,
    armed: bool,
    down: [bool; 12],
    presses: [Option<Press>; 12],
}
impl Keyboard {
    pub fn new(owner: Owner, epoch: u64) -> Result<Self, Refusal> {
        if !owner.valid() || epoch == 0 {
            return Err(Refusal::Invalid);
        }
        Ok(Self {
            owner,
            epoch,
            serial: 0,
            armed: true,
            down: [false; 12],
            presses: [None; 12],
        })
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn owner(&self) -> Owner {
        self.owner
    }
    #[cfg(test)]
    pub fn seed_serial_for_test(&mut self, serial: u64) {
        self.serial = serial;
    }
    pub fn press_state(&self, key: &str) -> Option<PressState> {
        self.presses[KEYS
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(key))?]
        .map(|press| press.state)
    }
    pub fn key_down(
        &mut self,
        key: &str,
        repeat: bool,
        focus: Focus,
        transport: Transport,
        admitted: bool,
    ) -> Result<Option<Action>, Refusal> {
        let Some(index) = KEYS
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(key))
        else {
            return Ok(None);
        };
        if repeat || self.down[index] {
            return Ok(None);
        }
        self.down[index] = true;
        if !self.armed || !focus.allows_on() {
            return Ok(None);
        }
        if transport != Transport::Playing {
            return Err(Refusal::NotPlaying);
        }
        if !admitted {
            return Err(Refusal::NotAdmitted);
        }
        let serial = self
            .serial
            .checked_add(1)
            .ok_or(Refusal::IdentityExhausted)?;
        let action = Action::On {
            owner: self.owner,
            token: Token {
                epoch: self.epoch,
                serial,
            },
            key: index as u8,
            pitch: 60 + index as u8,
            velocity: 100,
        };
        self.serial = serial;
        self.presses[index] = Some(Press {
            action,
            state: PressState::Offered,
        });
        Ok(Some(action))
    }
    /// Capture owned releases regardless of current text focus or selection.
    pub fn key_up(&mut self, key: &str) -> Option<Action> {
        let index = KEYS
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(key))?;
        self.down[index] = false;
        let press = self.presses[index].take()?;
        Some(Action::Off {
            owner: self.owner,
            token: press.action.token(),
        })
    }
    /// A queue receipt is not an audio-application acknowledgement. `None`
    /// means queue refusal; release refusal requires an external watermark.
    pub fn queue_result(&mut self, action: Action, ticket: Option<u64>) -> Option<Cut> {
        match action {
            Action::On { key, .. } => {
                if let Some(slot) = self.presses.get_mut(usize::from(key))
                    && slot.is_some_and(|press| {
                        press.action == action && press.state == PressState::Offered
                    })
                {
                    if let Some(ticket) =
                        ticket.filter(|ticket| *ticket != 0 && *ticket != u64::MAX)
                    {
                        *slot = Some(Press {
                            action,
                            state: PressState::Queued(ticket),
                        });
                    } else {
                        *slot = None;
                    }
                }
                None
            }
            Action::Off { owner, token }
                if ticket.is_none_or(|ticket| ticket == 0 || ticket == u64::MAX)
                    && owner == self.owner
                    && token.epoch == self.epoch =>
            {
                Some(self.cut(CutReason::ReleaseQueueFull))
            }
            Action::Off { .. } => None,
        }
    }
    pub fn result(&mut self, result: AppliedResult) {
        let Action::On { key, .. } = result.action else {
            return;
        };
        if let Some(slot) = self.presses.get_mut(usize::from(key))
            && slot.is_some_and(|press| {
                press.action == result.action && matches!(press.state, PressState::Queued(_))
            })
        {
            match result.kind {
                ResultKind::OnApplied => {
                    *slot = Some(Press {
                        action: result.action,
                        state: PressState::Applied,
                    })
                }
                ResultKind::Refused(_) => *slot = None,
                ResultKind::OffPending => {}
            }
        }
    }
    /// Publication of this cut is mandatory even at epoch exhaustion. No new
    /// arm is permitted until the consumer has observed it. Physical downs
    /// remain suppressed until key-up; cancellation does not create fresh downs.
    pub fn cut(&mut self, reason: CutReason) -> Cut {
        let cut = Cut {
            through_epoch: self.epoch,
            reason,
        };
        self.armed = false;
        self.presses = [None; 12];
        if let Some(next) = self.epoch.checked_add(1) {
            self.epoch = next;
        }
        cut
    }
    pub fn rearm(&mut self, owner: Owner, observed_through_epoch: u64) -> Result<(), Refusal> {
        if self.armed {
            return Err(Refusal::Stale);
        }
        if !owner.valid() {
            return Err(Refusal::Invalid);
        }
        if self.epoch <= observed_through_epoch {
            return Err(Refusal::IdentityExhausted);
        }
        // The acknowledgement must cover the immediately previous arm, rather
        // than merely reporting an arbitrary old cancellation.
        if observed_through_epoch != self.epoch - 1 {
            return Err(Refusal::Stale);
        }
        self.owner = owner;
        self.armed = true;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    Held,
    ReleasePending,
    Releasing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Voice {
    identity: Identity,
    gate: Gate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub voice_limit: u8,
    pub other_voices: u8,
    pub events_remaining: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseBatch {
    pub identities: [Option<Identity>; MAX_VOICES],
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ledger {
    owner: Owner,
    epoch: u64,
    cancelled_through: u64,
    last_ticket: u64,
    voices: [Option<Voice>; MAX_VOICES],
}
impl Ledger {
    pub fn new(owner: Owner, epoch: u64, cancelled_through: u64) -> Result<Self, Refusal> {
        if !owner.valid() || epoch == 0 || epoch <= cancelled_through {
            return Err(Refusal::Invalid);
        }
        Ok(Self {
            owner,
            epoch,
            cancelled_through,
            last_ticket: 0,
            voices: [None; MAX_VOICES],
        })
    }
    pub fn occupied(&self) -> usize {
        self.voices.iter().flatten().count()
    }
    pub fn rearm(&mut self, owner: Owner, epoch: u64) -> Result<(), Refusal> {
        if !owner.valid()
            || epoch <= self.epoch
            || epoch <= self.cancelled_through
            || self.epoch > self.cancelled_through
        {
            return Err(Refusal::Stale);
        }
        self.owner = owner;
        self.epoch = epoch;
        Ok(())
    }
    /// Reference state-machine acknowledgement only: no native runtime has
    /// applied this action. Future consumers must acknowledge their transaction.
    pub fn apply(
        &mut self,
        action: Action,
        ticket: u64,
        transport: Transport,
        admitted: bool,
        budget: Budget,
    ) -> AppliedResult {
        let refusal = if ticket == 0 || ticket == u64::MAX {
            Some(Refusal::Invalid)
        } else if ticket <= self.last_ticket {
            Some(Refusal::Stale)
        } else {
            None
        };
        if let Some(reason) = refusal {
            return AppliedResult {
                action,
                kind: ResultKind::Refused(reason),
            };
        }
        self.last_ticket = ticket;
        let token = action.token();
        let identity = action.identity();
        let kind = match action {
            Action::On {
                owner,
                key,
                pitch,
                velocity,
                ..
            } => {
                let refusal = if !owner.valid()
                    || token.serial == 0
                    || key >= 12
                    || pitch != 60 + key
                    || velocity != 100
                {
                    Some(Refusal::Invalid)
                } else if owner != self.owner
                    || token.epoch != self.epoch
                    || token.epoch <= self.cancelled_through
                    || self
                        .voices
                        .iter()
                        .flatten()
                        .any(|voice| voice.identity == identity)
                {
                    Some(Refusal::Stale)
                } else if transport != Transport::Playing {
                    Some(Refusal::NotPlaying)
                } else if !admitted {
                    Some(Refusal::NotAdmitted)
                } else if !matches!(budget.voice_limit, 16 | 64)
                    || usize::from(budget.other_voices) + self.occupied()
                        >= usize::from(budget.voice_limit)
                    || budget.events_remaining == 0
                    || budget.events_remaining > 256
                {
                    Some(Refusal::Capacity)
                } else {
                    None
                };
                if let Some(reason) = refusal {
                    ResultKind::Refused(reason)
                } else if let Some(slot) = self.voices.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(Voice {
                        identity,
                        gate: Gate::Held,
                    });
                    ResultKind::OnApplied
                } else {
                    ResultKind::Refused(Refusal::Capacity)
                }
            }
            Action::Off { .. } => {
                if let Some(voice) = self
                    .voices
                    .iter_mut()
                    .flatten()
                    .find(|voice| voice.identity == identity)
                {
                    if voice.gate == Gate::Held {
                        voice.gate = Gate::ReleasePending;
                    }
                    ResultKind::OffPending
                } else {
                    ResultKind::Refused(Refusal::Stale)
                }
            }
        };
        AppliedResult { action, kind }
    }
    /// Trusted external watermark is applied before the queue prefix. No new
    /// onset from a cancelled epoch can be resurrected, in either arrival order.
    pub fn cancel(&mut self, cut: Cut) {
        self.cancelled_through = self.cancelled_through.max(cut.through_epoch);
        for slot in &mut self.voices {
            if let Some(voice) = slot {
                let Identity::Live { token, .. } = voice.identity else {
                    continue;
                };
                if token.epoch <= cut.through_epoch {
                    if matches!(
                        cut.reason,
                        CutReason::Stop | CutReason::Seek | CutReason::Reset
                    ) {
                        *slot = None;
                    } else if voice.gate == Gate::Held {
                        voice.gate = Gate::ReleasePending;
                    }
                }
            }
        }
    }
    /// Pause leaves releases pending. On resume, return them before any sample;
    /// the caller must acknowledge dispatched releases separately.
    pub fn pending_releases(&self, transport: Transport) -> ReleaseBatch {
        let mut batch = ReleaseBatch {
            identities: [None; MAX_VOICES],
            len: 0,
        };
        if transport == Transport::Playing {
            for voice in self
                .voices
                .iter()
                .flatten()
                .filter(|voice| voice.gate == Gate::ReleasePending)
            {
                batch.identities[batch.len] = Some(voice.identity);
                batch.len += 1;
            }
        }
        batch
    }
    pub fn release_applied(&mut self, identity: Identity) -> bool {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .flatten()
            .find(|voice| voice.identity == identity && voice.gate == Gate::ReleasePending)
        {
            voice.gate = Gate::Releasing;
            true
        } else {
            false
        }
    }
    /// Only an actual owner retirement acknowledgement frees the reservation.
    pub fn retired(&mut self, identity: Identity) -> bool {
        if let Some(slot) = self.voices.iter_mut().find(|slot| {
            slot.is_some_and(|voice| voice.identity == identity && voice.gate == Gate::Releasing)
        }) {
            *slot = None;
            true
        } else {
            false
        }
    }
}
