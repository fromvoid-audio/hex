//! Readiness belongs to the new control's counters, never an old stream's tickets.
use super::audio_admission::ProjectRevision;
use hex_session::service::Request;

pub(super) struct DeferredNativeEffect {
    pub(super) project: ProjectRevision,
    pub(super) request: Box<Request>,
}
#[derive(Default)]
pub(super) struct RecoveryState {
    next_generation: u64,
    active: Option<Attempt>,
    pub(super) native_effect: Option<DeferredNativeEffect>,
}

struct Attempt {
    generation: u64,
    position: u64,
    opened: bool,
    restored: bool,
    seek_ticket: Option<u64>,
    seek_outcome: Option<hex_audio::prepared::ActionOutcome>,
    prepared_seek: Option<u64>,
    prepared_revision: Option<u64>,
}

impl RecoveryState {
    pub(super) fn begin(&mut self, position: u64) -> Result<u64, &'static str> {
        if self.active.is_some() {
            return Err("audio recovery is already pending");
        }
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or("audio recovery generation exhausted")?;
        self.next_generation = generation;
        self.native_effect = None;
        self.active = Some(Attempt {
            generation,
            position,
            opened: false,
            restored: false,
            seek_ticket: None,
            seek_outcome: None,
            prepared_seek: None,
            prepared_revision: None,
        });
        Ok(generation)
    }

    pub(super) fn generation(&self) -> Option<u64> {
        self.active.as_ref().map(|attempt| attempt.generation)
    }

    pub(super) fn position(&self) -> Option<u64> {
        self.active.as_ref().map(|attempt| attempt.position)
    }

    pub(super) fn opened(&mut self, generation: u64) -> bool {
        let Some(attempt) = self
            .active
            .as_mut()
            .filter(|attempt| attempt.generation == generation)
        else {
            return false;
        };
        attempt.opened = true;
        true
    }

    pub(super) fn restored(&mut self, generation: u64) -> bool {
        let Some(attempt) = self
            .active
            .as_mut()
            .filter(|attempt| attempt.generation == generation && attempt.opened)
        else {
            return false;
        };
        attempt.restored = true;
        true
    }

    pub(super) fn seek_needed(&self, graph_is_current: bool) -> Option<u64> {
        self.active
            .as_ref()
            .filter(|attempt| {
                attempt.opened
                    && attempt.restored
                    && attempt.seek_ticket.is_none()
                    && attempt.prepared_seek.is_none()
                    && graph_is_current
            })
            .map(|attempt| attempt.position)
    }

    pub(super) fn seeking(&mut self, ticket: u64) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.seek_ticket = Some(ticket);
            attempt.seek_outcome = None;
        }
    }

    pub(super) fn observe_receipt(&mut self, receipt: hex_audio::prepared::ActionOutcomeReceipt) {
        if let Some(attempt) = self.active.as_mut()
            && attempt.seek_ticket == Some(receipt.ticket)
        {
            attempt.seek_outcome = Some(receipt.outcome);
        }
    }

    pub(super) fn seek_failed(&self) -> bool {
        self.active
            .as_ref()
            .and_then(|attempt| attempt.seek_outcome)
            .is_some_and(|outcome| outcome != hex_audio::prepared::ActionOutcome::Applied)
    }

    /// Only this recovery attempt may admit a stopped prepared cursor restore. User seeks and
    /// stale attempts retain their ordinary recovery barrier.
    pub(super) fn permits_seek(&self, generation: Option<u64>) -> bool {
        match (&self.active, generation) {
            (None, None) => true,
            (Some(attempt), Some(generation)) => {
                attempt.generation == generation && attempt.opened && attempt.restored
            }
            _ => false,
        }
    }

    pub(super) fn prepared_seeking(&mut self, generation: u64, request: u64) -> bool {
        let Some(attempt) = self.active.as_mut().filter(|a| {
            a.generation == generation
                && a.opened
                && a.restored
                && a.seek_ticket.is_none()
                && a.prepared_seek.is_none()
        }) else {
            return false;
        };
        attempt.prepared_seek = Some(request);
        true
    }

    pub(super) fn prepared_request(&self) -> Option<u64> {
        self.active.as_ref()?.prepared_seek
    }

    pub(super) fn prepared_published(&mut self, request: u64, revision: u64) {
        if let Some(attempt) = self
            .active
            .as_mut()
            .filter(|a| a.prepared_seek == Some(request))
        {
            attempt.prepared_revision = Some(revision);
        }
    }

    pub(super) fn prepared_revision(&self) -> Option<u64> {
        self.active.as_ref()?.prepared_revision
    }

    /// Exact tagged view metadata plus this output's actual applied counter establishes restore.
    /// Neither a submitted graph nor a command ticket proves a prepared seek was installed.
    pub(super) fn complete_prepared(
        &mut self,
        graph_is_current: bool,
        audible: Option<(u64, u64)>,
        applied_revision: u64,
        position: u64,
        playing: bool,
    ) -> bool {
        let ready = self.active.as_ref().is_some_and(|attempt| {
            attempt.opened
                && attempt.restored
                && graph_is_current
                && !playing
                && position == attempt.position
                && audible.is_some_and(|(request, revision)| {
                    attempt.prepared_seek == Some(request) && revision == applied_revision
                })
        });
        if ready {
            self.active = None;
        }
        ready
    }

    pub(super) fn complete(
        &mut self,
        graph_is_current: bool,
        consumed_ticket: u64,
        position: u64,
        playing: bool,
    ) -> bool {
        let ready = self.active.as_ref().is_some_and(|attempt| {
            attempt.opened
                && attempt.restored
                && graph_is_current
                && !playing
                && position == attempt.position
                && attempt
                    .seek_ticket
                    .is_some_and(|ticket| consumed_ticket >= ticket)
        });
        if ready {
            self.active = None;
        }
        ready
    }

    pub(super) fn complete_exact(
        &mut self,
        graph_is_current: bool,
        position: u64,
        playing: bool,
    ) -> bool {
        let ready = self.active.as_ref().is_some_and(|attempt| {
            attempt.opened
                && attempt.restored
                && graph_is_current
                && !playing
                && position == attempt.position
                && attempt.seek_ticket.is_some()
                && attempt.seek_outcome == Some(hex_audio::prepared::ActionOutcome::Applied)
        });
        if ready {
            self.active = None;
        }
        ready
    }

    pub(super) fn stop(&mut self) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.position = 0;
            attempt.seek_ticket = None;
            attempt.seek_outcome = None;
            attempt.prepared_seek = None;
            attempt.prepared_revision = None;
        }
    }

    pub(super) fn fail(&mut self) {
        self.active = None;
        self.native_effect = None;
    }
}
