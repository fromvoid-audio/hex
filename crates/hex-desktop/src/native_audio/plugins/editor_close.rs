//! One close decision owns the hold requests for the current hosted bindings.
//! The control lane keeps each request until its exact reply or source retirement.
use super::*;
use hex_audio::{
    plugin_worker::{
        ControllerRequest, EditorAdmissionState, NativeWorkerControllerProgress as Progress,
        NativeWorkerControllerQuery,
    },
    prepared::PublicationControl,
};
use hex_session::service::{
    PluginEditorCloseAdmission as Admission, PluginEditorCloseAttempt, RequestCancellation,
};
use std::{num::NonZeroU64, sync::Weak};

pub(super) struct EditorClose {
    owner: Weak<PluginEditorCloseAttempt>,
    generation: u64,
    output: u64,
    ordinal: NonZeroU64,
    workers: Vec<Worker>,
}

pub(super) struct Worker {
    binding: Binding,
    stage: Stage,
    saved_admitted_through: u64,
}

/// The exact successor's temporary producer hold during replay. The release
/// continues on the usual control poll even when capture or publication fails.
pub(super) struct ReplayEditorHold {
    worker: Worker,
    ordinal: NonZeroU64,
    releasing: bool,
}

enum Stage {
    Pending,
    Holding(NativeWorkerControllerQuery),
    Held(u64),
    NeedsRelease,
    Releasing(NativeWorkerControllerQuery),
    Finished,
}

impl Worker {
    /// Output close transfers the exact pending admission reply with its
    /// original edit reader. Root consumes it before issuing the final seal.
    fn retire_on_output_close(&mut self, sources: &mut [Source]) {
        if !matches!(self.stage, Stage::Holding(_) | Stage::Releasing(_)) {
            self.stage = Stage::Finished;
            return;
        }
        let Some(source) = sources.iter_mut().find(|source| {
            source.binding == self.binding && source.editor_retirement.lease.is_some()
        }) else {
            return;
        };
        if source.editor_retirement.pending_controller.is_some() {
            return;
        }
        let previous = std::mem::replace(&mut self.stage, Stage::Finished);
        if let Stage::Holding(query) | Stage::Releasing(query) = previous {
            source.editor_retirement.pending_controller = Some(query);
        }
    }

    /// A refusal aborts acquisition for this attempt. Any possible native hold
    /// stays in the release path until its producer supplies a terminal.
    fn poll(
        &mut self,
        control: &mut PublicationControl,
        ordinal: NonZeroU64,
        cancel: bool,
    ) -> bool {
        if cancel {
            match self.stage {
                Stage::Pending => self.stage = Stage::Finished,
                Stage::Held(_) => self.stage = Stage::NeedsRelease,
                _ => {}
            }
        }
        self.stage = match &self.stage {
            Stage::Pending => {
                match control.begin_native_source_controller_query_on_control(
                    self.binding,
                    ControllerRequest::HoldEditorAdmission(ordinal),
                ) {
                    Ok(query) => Stage::Holding(query),
                    Err(reason) if busy(reason) => return true,
                    Err(_) => {
                        self.stage = Stage::Finished;
                        return false;
                    }
                }
            }
            Stage::Holding(query) => {
                match control.poll_native_source_controller_query_on_control(self.binding, query) {
                    Ok(Progress::Pending) => return true,
                    Err(reason) if busy(reason) => return true,
                    Ok(Progress::EditorAdmission(EditorAdmissionState::Held {
                        admitted_through,
                    })) => {
                        if cancel {
                            Stage::NeedsRelease
                        } else {
                            Stage::Held(admitted_through)
                        }
                    }
                    Ok(Progress::EditorAdmission(EditorAdmissionState::Refused)) => {
                        if cancel {
                            Stage::Finished
                        } else {
                            Stage::Pending
                        }
                    }
                    // A failed poll cannot prove that the main lane left
                    // admission open. Settle it with the same owner token.
                    _ => {
                        self.stage = Stage::NeedsRelease;
                        return false;
                    }
                }
            }
            Stage::NeedsRelease => match control.begin_native_source_controller_query_on_control(
                self.binding,
                ControllerRequest::ReleaseEditorAdmission(ordinal),
            ) {
                Ok(query) => Stage::Releasing(query),
                Err(_) => return true,
            },
            Stage::Releasing(query) => {
                match control.poll_native_source_controller_query_on_control(self.binding, query) {
                    Ok(Progress::Pending) => return true,
                    Err(reason) if busy(reason) => return true,
                    Ok(Progress::EditorAdmission(
                        EditorAdmissionState::Released | EditorAdmissionState::Refused,
                    )) => Stage::Finished,
                    _ => Stage::NeedsRelease,
                }
            }
            Stage::Held(_) | Stage::Finished => return true,
        };
        true
    }
}

fn busy(reason: &str) -> bool {
    matches!(
        reason,
        "native preparation is busy"
            | "native source is busy"
            | "native controller mailbox is busy"
            | "native controller query is already pending"
            | "native capture source is not active"
            | "native capture requires original active effect custody"
    )
}

fn current_source(source: &Source, output: u64, generation: u64) -> bool {
    source.binding.root == output
        && source.project_generation == generation
        && source.accepted
        && !source.retiring
}

impl Plugins {
    pub(super) fn hold_editor_for_replay(
        &mut self,
        control: &mut PublicationControl,
        binding: Binding,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<(), String> {
        self.poll_replay_editor_hold(control);
        if self.replay_editor_hold.is_some() || self.editor_close.is_some() {
            return Err("vst3 replacement editor admission is busy".into());
        }
        let ordinal = self
            .next_editor_hold_ordinal()
            .ok_or("vst3 replacement editor hold identity exhausted")?;
        let hold = self.replay_editor_hold.insert(ReplayEditorHold {
            worker: Worker {
                binding,
                stage: Stage::Pending,
                saved_admitted_through: 0,
            },
            ordinal,
            releasing: false,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        loop {
            edits::check_cancel(cancellation)?;
            if !hold.worker.poll(control, ordinal, false) {
                return Err("vst3 replacement editor hold was refused".into());
            }
            match hold.worker.stage {
                Stage::Held(0) => return Ok(()),
                Stage::Held(_) => {
                    return Err("vst3 replacement accepted a newer editor edit".into());
                }
                _ if std::time::Instant::now() >= deadline => {
                    return Err("vst3 replacement editor is still active".into());
                }
                _ => edits::poll_control(control),
            }
        }
    }

    pub(super) fn replay_editor_is_held(&self, binding: Binding) -> bool {
        self.replay_editor_hold.as_ref().is_some_and(|hold| {
            hold.worker.binding == binding
                && matches!(hold.worker.stage, Stage::Held(0))
                && !hold.releasing
        })
    }

    pub(super) fn release_replay_editor_hold(&mut self, control: Option<&mut PublicationControl>) {
        if let Some(hold) = self.replay_editor_hold.as_mut() {
            hold.releasing = true;
        }
        if let Some(control) = control {
            self.poll_replay_editor_hold(control);
        }
    }

    fn poll_replay_editor_hold(&mut self, control: &mut PublicationControl) {
        let Some(hold) = &mut self.replay_editor_hold else {
            return;
        };
        let binding = hold.worker.binding;
        if control.output_identity() != Some(binding.root)
            || !self
                .sources
                .iter()
                .any(|source| source.binding == binding && !source.retiring)
        {
            // Exact endpoint custody has returned to Root retirement.
            self.replay_editor_hold = None;
        } else if hold.releasing {
            hold.worker.poll(control, hold.ordinal, true);
            if matches!(hold.worker.stage, Stage::Finished) {
                self.replay_editor_hold = None;
            }
        }
    }

    fn next_editor_hold_ordinal(&mut self) -> Option<NonZeroU64> {
        let ordinal = NonZeroU64::new(self.editor_close_ordinal.checked_add(1)?)?;
        self.editor_close_ordinal = ordinal.get();
        Some(ordinal)
    }

    pub(in crate::native_audio) fn prepare_editor_close(
        &mut self,
        control: Option<&mut PublicationControl>,
        generation: u64,
        owner: &Weak<PluginEditorCloseAttempt>,
    ) -> Admission {
        if !owner.upgrade().is_some_and(|owner| owner.is_valid()) {
            return Admission::Refused;
        }
        // Restoration will return each original controller and reuse its
        // helper under a new binding. Wait for the new source set.
        if self.restore.is_some() {
            return Admission::Pending;
        }
        let Some(control) = control else {
            return if self.sources.iter().any(|source| {
                source.project_generation == generation && source.accepted && !source.retiring
            }) {
                Admission::Pending
            } else {
                Admission::Held {
                    admitted_edits: false,
                }
            };
        };
        self.poll_editor_close(control, Some(generation));
        if self.replay_editor_hold.is_some() {
            return Admission::Pending;
        }
        if let Some(attempt) = &self.editor_close {
            return if Weak::ptr_eq(&attempt.owner, owner) {
                attempt.admission()
            } else {
                Admission::Pending
            };
        }

        let Some(output) = control.output_identity() else {
            return Admission::Pending;
        };
        let accepted = self
            .sources
            .iter()
            .filter(|source| current_source(source, output, generation));
        let count = accepted.clone().count();
        let mut workers = Vec::new();
        if workers.try_reserve_exact(count).is_err() {
            return Admission::Refused;
        }
        workers.extend(accepted.map(|source| Worker {
            binding: source.binding,
            stage: Stage::Pending,
            saved_admitted_through: self.saved_editor_frontier(source.binding),
        }));
        let Some(ordinal) = self.next_editor_hold_ordinal() else {
            return Admission::Refused;
        };
        self.editor_close = Some(EditorClose {
            owner: owner.clone(),
            generation,
            output,
            ordinal,
            workers,
        });
        self.poll_editor_close(control, Some(generation));
        if count == 0 {
            Admission::Held {
                admitted_edits: false,
            }
        } else {
            Admission::Pending
        }
    }

    pub(super) fn poll_editor_close(
        &mut self,
        control: &mut PublicationControl,
        generation: Option<u64>,
    ) {
        self.poll_replay_editor_hold(control);
        let Some(attempt) = &mut self.editor_close else {
            return;
        };
        if control.output_identity() != Some(attempt.output) {
            self.editor_close_output_closed();
            return;
        }
        let output = attempt.output;
        let accepted = self
            .sources
            .iter()
            .filter(|source| current_source(source, output, attempt.generation));
        let same_bindings = accepted
            .map(|source| source.binding)
            .eq(attempt.workers.iter().map(|worker| worker.binding));
        let mut cancel = generation.is_some_and(|generation| generation != attempt.generation)
            || !same_bindings
            || !attempt.is_valid();
        if cancel {
            attempt.invalidate();
        }
        for worker in &mut attempt.workers {
            if !same_bindings
                && !self.sources.iter().any(|source| {
                    source.binding == worker.binding
                        && current_source(source, output, attempt.generation)
                })
            {
                // The exact old worker is in Root retirement or has returned.
                // A replacement is never eligible to settle its hold.
                worker.stage = Stage::Finished;
            } else if !worker.poll(control, attempt.ordinal, cancel) {
                cancel = true;
            }
        }
        if cancel {
            attempt.invalidate();
        }
        if cancel
            && attempt
                .workers
                .iter()
                .all(|w| matches!(w.stage, Stage::Finished))
        {
            self.editor_close = None;
        }
    }
    pub(super) fn invalidate_editor_close(&self) {
        if let Some(attempt) = &self.editor_close {
            attempt.invalidate();
        }
    }

    pub(super) fn saved_editor_frontier(&self, binding: Binding) -> u64 {
        self.saved_editor_frontiers
            .iter()
            .find_map(|(candidate, seq)| (*candidate == binding).then_some(*seq))
            .unwrap_or(0)
    }

    pub(super) fn held_editor_close_frontier(&self, binding: Binding) -> Option<u64> {
        let attempt = self.editor_close.as_ref()?;
        if !attempt.is_valid() {
            return None;
        }
        attempt.workers.iter().find_map(|worker| {
            (worker.binding == binding)
                .then_some(match worker.stage {
                    Stage::Held(sequence) => Some(sequence),
                    _ => None,
                })
                .flatten()
        })
    }

    pub(super) fn record_saved_editor_frontier(&mut self, binding: Binding, frontier: u64) {
        if let Some((_, slot)) = self
            .saved_editor_frontiers
            .iter_mut()
            .find(|(candidate, _)| *candidate == binding)
        {
            *slot = (*slot).max(frontier);
        } else {
            self.saved_editor_frontiers.push((binding, frontier));
        }
        if let Some(attempt) = self.editor_close.as_mut() {
            for worker in &mut attempt.workers {
                if worker.binding == binding {
                    worker.saved_admitted_through = worker.saved_admitted_through.max(frontier);
                }
            }
        }
        let sources = &self.sources;
        self.saved_editor_frontiers.retain(|(candidate, _)| {
            sources.iter().any(|s| s.binding == *candidate) || *candidate == binding
        });
    }

    pub(super) fn editor_close_output_closed(&mut self) {
        self.saved_editor_frontiers.clear();
        self.pending_save_frontiers = None;
        if let Some(hold) = &mut self.replay_editor_hold {
            hold.worker.retire_on_output_close(&mut self.sources);
            if matches!(hold.worker.stage, Stage::Finished) {
                self.replay_editor_hold = None;
            }
        }
        if let Some(attempt) = &mut self.editor_close {
            attempt.invalidate();
            for worker in &mut attempt.workers {
                worker.retire_on_output_close(&mut self.sources);
            }
            if attempt
                .workers
                .iter()
                .all(|worker| matches!(worker.stage, Stage::Finished))
            {
                self.editor_close = None;
            }
        }
    }
}

impl EditorClose {
    fn is_valid(&self) -> bool {
        self.owner.upgrade().is_some_and(|owner| owner.is_valid())
    }

    fn invalidate(&self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.invalidate();
        }
    }

    fn admission(&self) -> Admission {
        if !self.is_valid() {
            Admission::Refused
        } else if self
            .workers
            .iter()
            .all(|worker| matches!(worker.stage, Stage::Held(_)))
        {
            Admission::Held {
                admitted_edits: self.workers.iter().any(|worker| {
                    matches!(
                        worker.stage,
                        Stage::Held(sequence) if sequence > worker.saved_admitted_through
                    )
                }),
            }
        } else {
            Admission::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(instance: u64, epoch: u64) -> Binding {
        Binding {
            root: 1,
            graph: 1,
            instance,
            epoch,
        }
    }

    fn held_attempt(
        owner: &Arc<PluginEditorCloseAttempt>,
        binding: Binding,
        sequence: u64,
        saved_admitted_through: u64,
    ) -> EditorClose {
        EditorClose {
            owner: Arc::downgrade(owner),
            generation: 7,
            output: 1,
            ordinal: NonZeroU64::new(1).unwrap(),
            workers: vec![Worker {
                binding,
                stage: Stage::Held(sequence),
                saved_admitted_through,
            }],
        }
    }

    #[test]
    fn saved_frontier_clears_close_warning_until_a_newer_editor_edit_arrives() {
        let owner = Arc::new(PluginEditorCloseAttempt::default());
        let b = binding(10, 1);
        let context = AudioSourceContext {
            project_generation: 7,
            editor_revision: 3,
        };
        let mut plugins = Plugins {
            editor_close: Some(held_attempt(&owner, b, 5, 0)),
            ..Default::default()
        };

        // Before save, sequence 5 > saved 0 warns about unsaved VST3 edits.
        assert_eq!(
            plugins.editor_close.as_ref().unwrap().admission(),
            Admission::Held {
                admitted_edits: true
            }
        );

        // Failed or cancelled save must NOT advance the saved frontier.
        plugins.pending_save_frontiers = Some((7, vec![(b, 5)]));
        plugins.finish_state_save(context, false);
        assert_eq!(plugins.saved_editor_frontier(b), 0);
        assert_eq!(
            plugins.editor_close.as_ref().unwrap().admission(),
            Admission::Held {
                admitted_edits: true
            }
        );

        // Clean save advances saved frontier to 5 -> no false quit warning!
        plugins.pending_save_frontiers = Some((7, vec![(b, 5)]));
        plugins.finish_state_save(context, true);
        assert_eq!(plugins.saved_editor_frontier(b), 5);
        assert_eq!(
            plugins.editor_close.as_ref().unwrap().admission(),
            Admission::Held {
                admitted_edits: false
            }
        );

        // A later close check with sequence == 5 still reports clean; sequence == 6 warns again.
        let later_owner = Arc::new(PluginEditorCloseAttempt::default());
        let clean = held_attempt(&later_owner, b, 5, plugins.saved_editor_frontier(b));
        assert_eq!(
            clean.admission(),
            Admission::Held {
                admitted_edits: false
            }
        );
        let edited_again = held_attempt(&later_owner, b, 6, plugins.saved_editor_frontier(b));
        assert_eq!(
            edited_again.admission(),
            Admission::Held {
                admitted_edits: true
            }
        );
    }
}
