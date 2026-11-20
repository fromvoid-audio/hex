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
use hex_session::service::{PluginEditorCloseAdmission as Admission, PluginEditorCloseAttempt};
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
        let Some(ordinal) = self
            .editor_close_ordinal
            .checked_add(1)
            .and_then(NonZeroU64::new)
        else {
            return Admission::Refused;
        };
        let mut workers = Vec::new();
        if workers.try_reserve_exact(count).is_err() {
            return Admission::Refused;
        }
        workers.extend(accepted.map(|source| Worker {
            binding: source.binding,
            stage: Stage::Pending,
        }));
        self.editor_close_ordinal = ordinal.get();
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

    pub(super) fn editor_close_output_closed(&mut self) {
        if let Some(attempt) = self.editor_close.take() {
            attempt.invalidate();
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
                admitted_edits: self
                    .workers
                    .iter()
                    .any(|worker| matches!(worker.stage, Stage::Held(sequence) if sequence != 0)),
            }
        } else {
            Admission::Pending
        }
    }
}
