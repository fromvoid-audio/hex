//! Bounded, control-side display observations; terminals remain authoritative.
use super::{ArrangementEditViewKind, ArrangementEditViewObservation};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc::TryRecvError},
};

const CAPACITY: usize = 2;

struct State {
    queue: VecDeque<ArrangementEditViewObservation>,
    generation: Option<u64>,
    closed: bool,
}

pub(super) struct Sender(Arc<Mutex<State>>);
pub(super) struct Receiver(Arc<Mutex<State>>);

pub(super) fn channel() -> (Sender, Receiver) {
    let state = Arc::new(Mutex::new(State {
        queue: VecDeque::with_capacity(CAPACITY),
        generation: None,
        closed: false,
    }));
    (Sender(Arc::clone(&state)), Receiver(state))
}

impl Sender {
    /// Only consecutive paste revisions coalesce; incompatible full slots fall
    /// back to the ordinary terminal. No callback touches this mailbox.
    pub(super) fn send(&self, mut observation: ArrangementEditViewObservation) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .generation
            .is_some_and(|generation| observation.project_generation < generation)
        {
            return;
        }
        if state.generation != Some(observation.project_generation) {
            state.queue.clear();
            state.generation = Some(observation.project_generation);
        }
        if let Some(last) = state.queue.back_mut()
            && matches!(last.kind, ArrangementEditViewKind::Paste)
            && matches!(observation.kind, ArrangementEditViewKind::Paste)
            && last.previous_revision.checked_add(1) == Some(observation.previous_revision)
        {
            observation.first_previous_revision = last.first_previous_revision;
            *last = observation;
            return;
        }
        if state.queue.len() < CAPACITY {
            state.queue.push_back(observation);
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }
}

impl Receiver {
    pub(super) fn try_recv(&self) -> Result<ArrangementEditViewObservation, TryRecvError> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.queue.pop_front() {
            Some(observation) => Ok(observation),
            None if state.closed => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use std::sync::Weak;

    fn observation(
        generation: u64,
        previous: u64,
        kind: ArrangementEditViewKind,
    ) -> ArrangementEditViewObservation {
        ArrangementEditViewObservation {
            project_generation: generation,
            first_previous_revision: previous,
            previous_revision: previous,
            kind,
            state: Weak::new(),
        }
    }

    fn duplicate() -> ArrangementEditViewKind {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let created = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap();
        assert!(matches!(created, Created::Track(_)));
        ArrangementEditViewKind::Duplicate(created)
    }

    #[test]
    fn duplicate_and_contiguous_pastes_keep_order_and_latest_span() {
        let (sender, receiver) = channel();
        sender.send(observation(1, 3, duplicate()));
        sender.send(observation(1, 4, ArrangementEditViewKind::Paste));
        sender.send(observation(1, 5, ArrangementEditViewKind::Paste));
        let first = receiver.try_recv().unwrap();
        assert!(matches!(first.kind, ArrangementEditViewKind::Duplicate(_)));
        let paste = receiver.try_recv().unwrap();
        assert!(matches!(paste.kind, ArrangementEditViewKind::Paste));
        assert_eq!(
            (paste.first_previous_revision, paste.previous_revision),
            (4, 5)
        );
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        drop(sender);
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn gaps_full_capacity_and_generation_replacement_are_safe() {
        let (sender, receiver) = channel();
        sender.send(observation(1, 1, ArrangementEditViewKind::Paste));
        sender.send(observation(1, 3, ArrangementEditViewKind::Paste));
        let before_gap = receiver.try_recv().unwrap();
        let after_gap = receiver.try_recv().unwrap();
        assert_eq!(
            (
                before_gap.first_previous_revision,
                before_gap.previous_revision
            ),
            (1, 1)
        );
        assert_eq!(
            (
                after_gap.first_previous_revision,
                after_gap.previous_revision
            ),
            (3, 3)
        );
        sender.send(observation(2, 8, ArrangementEditViewKind::Paste));
        sender.send(observation(2, 10, duplicate()));
        sender.send(observation(2, 11, ArrangementEditViewKind::Paste));
        let first = receiver.try_recv().unwrap();
        assert_eq!(
            (first.first_previous_revision, first.previous_revision),
            (8, 8)
        );
        assert!(matches!(
            receiver.try_recv().unwrap().kind,
            ArrangementEditViewKind::Duplicate(_)
        ));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        sender.send(observation(2, 12, ArrangementEditViewKind::Paste));
        sender.send(observation(3, 0, ArrangementEditViewKind::Paste));
        sender.send(observation(2, 13, ArrangementEditViewKind::Paste));
        let replacement = receiver.try_recv().unwrap();
        assert_eq!(
            (
                replacement.project_generation,
                replacement.previous_revision
            ),
            (3, 0)
        );
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }
}
