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
                    && graph_is_current
            })
            .map(|attempt| attempt.position)
    }

    pub(super) fn seeking(&mut self, ticket: u64) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.seek_ticket = Some(ticket);
        }
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

    pub(super) fn stop(&mut self) {
        if let Some(attempt) = self.active.as_mut() {
            attempt.position = 0;
            attempt.seek_ticket = None;
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
