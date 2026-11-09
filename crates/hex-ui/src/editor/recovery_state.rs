//! Readiness belongs to the new control's counters, never an old stream's tickets.
#[derive(Default)]
pub(super) struct RecoveryState {
    next_generation: u64,
    active: Option<Attempt>,
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
    }
}

#[cfg(test)]
mod tests {
    use super::RecoveryState;

    #[test]
    fn recovery_requires_exact_success_not_a_later_terminal() {
        use hex_audio::prepared::{ActionOutcome, ActionOutcomeReceipt};
        let mut state = RecoveryState::default();
        let generation = state.begin(64).unwrap();
        assert!(state.opened(generation));
        assert!(state.restored(generation));
        state.seeking(3);
        state.observe_receipt(ActionOutcomeReceipt {
            ticket: 4,
            outcome: ActionOutcome::Applied,
        });
        assert!(!state.complete_exact(true, 64, false));
        state.observe_receipt(ActionOutcomeReceipt {
            ticket: 3,
            outcome: ActionOutcome::Rejected(hex_engine::PlaybackActionRejection::OutputFailed),
        });
        assert!(state.seek_failed());
        assert!(!state.complete_exact(true, 64, false));
    }

    #[test]
    fn prepared_recovery_requires_exact_attempt_request_and_actual_stopped_ack() {
        let mut state = RecoveryState::default();
        let generation = state.begin(129).unwrap();
        assert!(!state.permits_seek(None));
        assert!(!state.permits_seek(Some(generation)));
        state.opened(generation);
        state.restored(generation);
        assert!(state.permits_seek(Some(generation)));
        assert!(!state.permits_seek(Some(generation + 1)));
        assert!(!state.prepared_seeking(generation + 1, 7));
        assert!(state.prepared_seeking(generation, 7));
        assert!(!state.prepared_seeking(generation, 8));
        assert_eq!(state.seek_needed(true), None);
        assert!(
            !state.complete(true, u64::MAX, 129, false),
            "command tickets cannot apply prepared recovery"
        );
        assert!(!state.complete_prepared(true, None, 9, 129, false));
        assert!(!state.complete_prepared(true, Some((8, 9)), 9, 129, false));
        assert!(!state.complete_prepared(true, Some((7, 9)), 8, 129, false));
        assert!(!state.complete_prepared(true, Some((7, 9)), 10, 129, false));
        assert!(!state.complete_prepared(true, Some((7, 9)), 9, 128, false));
        assert!(!state.complete_prepared(true, Some((7, 9)), 9, 129, true));
        assert!(!state.complete_prepared(false, Some((7, 9)), 9, 129, false));
        state.prepared_published(8, 10);
        assert_eq!(state.prepared_revision(), None);
        state.prepared_published(7, 9);
        assert_eq!(state.prepared_revision(), Some(9));
        assert!(state.complete_prepared(true, Some((7, 9)), 9, 129, false));
        assert!(state.permits_seek(None));
        assert!(!state.permits_seek(Some(generation)));

        let next = state.begin(129).unwrap();
        state.opened(next);
        state.restored(next);
        state.prepared_seeking(next, 11);
        state.prepared_published(11, 12);
        state.stop();
        assert_eq!(state.prepared_request(), None);
        assert_eq!(state.prepared_revision(), None);
        assert_eq!(state.seek_needed(true), Some(0));
        assert!(!state.complete_prepared(true, Some((11, 12)), 12, 129, false));
        state.seeking(13);
        assert!(state.complete(true, 13, 0, false));
    }

    #[test]
    fn obsolete_completion_and_old_acknowledgments_do_not_enable_play() {
        let mut state = RecoveryState::default();
        let first = state.begin(640).expect("first recovery");
        assert!(state.begin(640).is_err());
        state.fail();
        let current = state.begin(640).expect("retry");
        assert!(!state.opened(first));
        assert!(!state.restored(first));
        assert!(state.opened(current));
        assert!(state.restored(current));
        // A huge prior stream acknowledgment is insufficient without a new seek.
        assert!(!state.complete(true, 1000, 640, false));
        assert_eq!(state.seek_needed(true), Some(640));
        state.seeking(1);
        assert!(!state.complete(false, 1, 640, false));
        assert!(!state.complete(true, 0, 640, false));
        assert!(!state.complete(true, 1, 0, false));
        assert!(!state.complete(true, 1, 640, true));
        assert!(state.complete(true, 1, 640, false));
        assert!(state.generation().is_none());
    }

    #[test]
    fn stop_supersedes_saved_cursor_and_requires_a_fresh_seek() {
        let mut state = RecoveryState::default();
        let generation = state.begin(960).expect("recovery");
        state.opened(generation);
        state.restored(generation);
        state.seeking(1);
        state.stop();
        assert_eq!(state.position(), Some(0));
        assert!(!state.complete(true, 1, 960, false));
        assert_eq!(state.seek_needed(true), Some(0));
        state.seeking(2);
        assert!(!state.complete(true, 1, 0, false));
        assert!(state.complete(true, 2, 0, false));
    }
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
}
