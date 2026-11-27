//! Readiness belongs to the new control's counters, never an old stream's tickets.
use hex_project::ClipId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeferredEditKind {
    NativeEffect,
    PluginEffect,
    RemoveDevice,
    ToggleDeviceBypass,
    SampleDrop,
    TimeDuplicate,
    LoopRegion,
    ClipPlayStart,
    MixerControl,
    GraphEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekOutcome {
    Applied,
    Rejected,
}

pub struct RecoveryState<C, D> {
    next_generation: u64,
    active: Option<Attempt<C>>,
    pub deferred_edit: Option<D>,
}

impl<C, D> Default for RecoveryState<C, D> {
    fn default() -> Self {
        Self {
            next_generation: 0,
            active: None,
            deferred_edit: None,
        }
    }
}

const MAX_BUSY_SEEK_RETRIES: u8 = 4;

struct Attempt<C> {
    generation: u64,
    position: u64,
    opened: bool,
    restored: bool,
    seek_ticket: Option<u64>,
    seek_outcome: Option<SeekOutcome>,
    seek_position: Option<u64>,
    busy_seek_retries: u8,
    prepared_retry_used: bool,
    prepared_seek: Option<u64>,
    prepared_revision: Option<u64>,
    clip_play_start: Option<(C, ClipId)>,
    mixer_edit: Option<C>,
}

impl<C: Copy + PartialEq, D> RecoveryState<C, D> {
    pub fn bind_clip_play_start(&mut self, context: C, clip: ClipId) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.clip_play_start = Some((context, clip));
        }
    }

    pub fn admits_clip_play_start(&self, context: C, clip: ClipId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|attempt| attempt.clip_play_start == Some((context, clip)))
    }

    pub fn bind_mixer_edit(&mut self, context: C) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.mixer_edit = Some(context);
        }
    }

    pub fn admits_mixer_edit(&self, context: C) -> bool {
        self.active
            .as_ref()
            .is_some_and(|attempt| attempt.mixer_edit == Some(context))
    }

    pub fn begin(&mut self, position: u64) -> Result<u64, &'static str> {
        if self.active.is_some() {
            return Err("audio recovery is already pending");
        }
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or("audio recovery generation exhausted")?;
        self.next_generation = generation;
        self.deferred_edit = None;
        self.active = Some(Attempt {
            generation,
            position,
            opened: false,
            restored: false,
            seek_ticket: None,
            seek_outcome: None,
            seek_position: None,
            busy_seek_retries: 0,
            prepared_retry_used: false,
            prepared_seek: None,
            prepared_revision: None,
            clip_play_start: None,
            mixer_edit: None,
        });
        Ok(generation)
    }

    pub fn generation(&self) -> Option<u64> {
        self.active.as_ref().map(|attempt| attempt.generation)
    }

    pub fn position(&self) -> Option<u64> {
        self.active.as_ref().map(|attempt| attempt.position)
    }

    /// Retain the latest user cursor without replacing an admitted request's custody.
    pub fn retarget(&mut self, position: u64) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.position = position;
            attempt.prepared_retry_used = false;
        }
    }

    pub fn opened(&mut self, generation: u64) -> bool {
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

    pub fn restored(&mut self, generation: u64) -> bool {
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

    pub fn seek_needed(&self, graph_is_current: bool) -> Option<u64> {
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

    pub fn seeking(&mut self, ticket: u64) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.seek_ticket = Some(ticket);
            attempt.seek_position = Some(attempt.position);
            attempt.seek_outcome = None;
        }
    }

    pub fn observe_receipt(&mut self, ticket: u64, outcome: SeekOutcome) {
        if let Some(attempt) = self.active.as_mut()
            && attempt.seek_ticket == Some(ticket)
        {
            attempt.seek_outcome = Some(outcome);
        }
    }

    /// Re-arm the recovery seek after a transient `StreamActionBusy` rejection
    /// (for example when a post-restart publication landed in the same block),
    /// up to a bounded retry count so a permanently wedged stream still fails.
    pub fn retry_busy_seek(&mut self, ticket: u64) -> bool {
        let Some(attempt) = self.active.as_mut() else {
            return false;
        };
        if attempt.seek_ticket != Some(ticket) || attempt.busy_seek_retries >= MAX_BUSY_SEEK_RETRIES
        {
            return false;
        }
        attempt.busy_seek_retries += 1;
        attempt.seek_ticket = None;
        attempt.seek_position = None;
        attempt.seek_outcome = None;
        true
    }

    pub fn seek_failed(&self) -> bool {
        self.active
            .as_ref()
            .and_then(|attempt| attempt.seek_outcome)
            .is_some_and(|outcome| outcome != SeekOutcome::Applied)
    }

    /// Only this recovery attempt may admit a stopped prepared cursor restore. User seeks and
    /// stale attempts retain their ordinary recovery barrier.
    pub fn permits_seek(&self, generation: Option<u64>) -> bool {
        match (&self.active, generation) {
            (None, None) => true,
            (Some(attempt), Some(generation)) => {
                attempt.generation == generation && attempt.opened && attempt.restored
            }
            _ => false,
        }
    }

    pub fn prepared_seeking(&mut self, generation: u64, request: u64) -> bool {
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
        attempt.seek_position = Some(attempt.position);
        true
    }

    /// An obsolete, unpublished preparation may be retried once for this cursor intent.
    pub fn retry_prepared_seek(&mut self, generation: u64, request: u64) -> bool {
        let Some(attempt) = self.active.as_mut().filter(|attempt| {
            attempt.generation == generation
                && attempt.prepared_seek == Some(request)
                && attempt.prepared_revision.is_none()
                && !attempt.prepared_retry_used
        }) else {
            return false;
        };
        attempt.prepared_retry_used = true;
        attempt.prepared_seek = None;
        attempt.seek_position = None;
        true
    }

    pub fn prepared_request(&self) -> Option<u64> {
        self.active.as_ref()?.prepared_seek
    }

    pub fn prepared_published(&mut self, request: u64, revision: u64) {
        if let Some(attempt) = self
            .active
            .as_mut()
            .filter(|a| a.prepared_seek == Some(request))
        {
            attempt.prepared_revision = Some(revision);
        }
    }

    pub fn prepared_revision(&self) -> Option<u64> {
        self.active.as_ref()?.prepared_revision
    }

    /// Exact tagged view metadata plus this output's actual applied counter establishes restore.
    /// Neither a submitted graph nor a command ticket proves a prepared seek was installed.
    pub fn complete_prepared(
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
                && Some(position) == attempt.seek_position
                && audible.is_some_and(|(request, revision)| {
                    attempt.prepared_seek == Some(request) && revision == applied_revision
                })
        });
        self.complete_cursor(ready)
    }

    pub fn complete(
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
                && Some(position) == attempt.seek_position
                && attempt
                    .seek_ticket
                    .is_some_and(|ticket| consumed_ticket >= ticket)
        });
        self.complete_cursor(ready)
    }

    pub fn complete_exact(&mut self, graph_is_current: bool, position: u64, playing: bool) -> bool {
        let ready = self.active.as_ref().is_some_and(|attempt| {
            attempt.opened
                && attempt.restored
                && graph_is_current
                && !playing
                && Some(position) == attempt.seek_position
                && attempt.seek_ticket.is_some()
                && attempt.seek_outcome == Some(SeekOutcome::Applied)
        });
        self.complete_cursor(ready)
    }

    fn complete_cursor(&mut self, ready: bool) -> bool {
        if !ready {
            return false;
        }
        let Some(attempt) = self.active.as_mut() else {
            return false;
        };
        if attempt.seek_position != Some(attempt.position) {
            // The old cursor is now acknowledged. The next tick may admit the
            // newest requested position against the newly installed runtime.
            attempt.seek_ticket = None;
            attempt.seek_outcome = None;
            attempt.seek_position = None;
            attempt.prepared_seek = None;
            attempt.prepared_revision = None;
            return false;
        }
        self.active = None;
        true
    }

    pub fn stop(&mut self) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.position = 0;
            attempt.seek_position = None;
            attempt.prepared_retry_used = false;
            attempt.seek_ticket = None;
            attempt.seek_outcome = None;
            attempt.prepared_seek = None;
            attempt.prepared_revision = None;
        }
    }

    pub fn fail(&mut self) {
        self.active = None;
        self.deferred_edit = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryState, SeekOutcome};

    #[test]
    fn recovery_requires_exact_success_not_a_later_terminal() {
        let mut state: RecoveryState<(), ()> = RecoveryState::default();
        let generation = state.begin(64).unwrap();
        assert!(state.opened(generation));
        assert!(state.restored(generation));
        state.seeking(3);
        state.observe_receipt(4, SeekOutcome::Applied);
        assert!(!state.complete_exact(true, 64, false));
        state.observe_receipt(3, SeekOutcome::Rejected);
        assert!(state.seek_failed());
        assert!(!state.complete_exact(true, 64, false));
    }

    #[test]
    fn prepared_recovery_requires_exact_attempt_request_and_actual_stopped_ack() {
        let mut state: RecoveryState<(), ()> = RecoveryState::default();
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
        let mut state: RecoveryState<(), ()> = RecoveryState::default();
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
        let mut state: RecoveryState<(), ()> = RecoveryState::default();
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
    fn transient_busy_seek_retries_up_to_bound_before_failing() {
        let mut state: RecoveryState<(), ()> = RecoveryState::default();
        let generation = state.begin(97_944).expect("recovery");
        state.opened(generation);
        state.restored(generation);

        for ticket in 1..=u64::from(super::MAX_BUSY_SEEK_RETRIES) {
            assert_eq!(state.seek_needed(true), Some(97_944));
            state.seeking(ticket);
            assert!(state.retry_busy_seek(ticket));
            assert!(!state.seek_failed());
        }

        // Next attempt succeeds cleanly on Applied receipt.
        let final_ticket = u64::from(super::MAX_BUSY_SEEK_RETRIES) + 1;
        assert_eq!(state.seek_needed(true), Some(97_944));
        state.seeking(final_ticket);
        // Exceeding the retry bound returns false so the caller records Rejected.
        assert!(!state.retry_busy_seek(final_ticket));
        state.observe_receipt(final_ticket, SeekOutcome::Rejected);
        assert!(state.seek_failed());
    }
}
