//! Bounded control-side switch observations; ordinary terminals remain authoritative.
use super::TrackSwitchViewObservation;
use std::sync::{Arc, Mutex, mpsc::TryRecvError};

struct State {
    observation: Option<TrackSwitchViewObservation>,
    generation: Option<u64>,
    closed: bool,
}

pub(super) struct Sender {
    state: Arc<Mutex<State>>,
    wake_hook: super::client::WakeHook,
}
pub(super) struct Receiver(Arc<Mutex<State>>);

pub(super) fn channel() -> (Sender, Receiver) {
    let state = Arc::new(Mutex::new(State {
        observation: None,
        generation: None,
        closed: false,
    }));
    (
        Sender {
            state: Arc::clone(&state),
            wake_hook: super::client::WakeHook::default(),
        },
        Receiver(state),
    )
}

impl Sender {
    pub(super) fn with_wake(mut self, wake_hook: super::client::WakeHook) -> Self {
        self.wake_hook = wake_hook;
        self
    }

    /// Coalesce only contiguous revisions. Gaps retain the earlier observation;
    /// the authoritative terminal remains available. No audio callback uses this mailbox.
    pub(super) fn send(&self, mut view: TrackSwitchViewObservation) {
        let updated = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state
                .generation
                .is_some_and(|generation| view.project_generation < generation)
            {
                return;
            }
            if state.generation != Some(view.project_generation) {
                state.observation = None;
                state.generation = Some(view.project_generation);
            }
            if let Some(previous) = state.observation.as_ref() {
                if previous.revision != view.previous_revision {
                    return;
                }
                view.first_previous_revision = previous.first_previous_revision;
            }
            state.observation = Some(view);
            true
        };
        if updated {
            self.wake_hook.wake();
        }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }
}
impl Receiver {
    pub(super) fn try_recv(&self) -> Result<TrackSwitchViewObservation, TryRecvError> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.observation.take() {
            Some(view) => Ok(view),
            None if state.closed => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view(generation: u64, previous: u64) -> TrackSwitchViewObservation {
        TrackSwitchViewObservation {
            project_generation: generation,
            first_previous_revision: previous,
            previous_revision: previous,
            revision: previous + 1,
            tracks: Vec::new(),
        }
    }
    #[test]
    fn contiguous_revisions_coalesce_but_gaps_preserve_the_first() {
        let (tx, rx) = channel();
        tx.send(view(1, 2));
        tx.send(view(1, 3));
        tx.send(view(1, 5));
        let actual = rx.try_recv().unwrap();
        assert_eq!(
            (
                actual.first_previous_revision,
                actual.previous_revision,
                actual.revision
            ),
            (2, 3, 4)
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        drop(tx);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
    }
    #[test]
    fn replacement_discards_old_generation_and_ignores_stale_send() {
        let (tx, rx) = channel();
        tx.send(view(1, 2));
        tx.send(view(2, 0));
        tx.send(view(1, 3));
        let actual = rx.try_recv().unwrap();
        assert_eq!((actual.project_generation, actual.revision), (2, 1));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }
}
