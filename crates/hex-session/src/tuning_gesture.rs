//! Bounded project-tuning edit protocol. The worker owns the transient
//! prepared overlay; only Commit changes canonical project history.
use crate::{Session, SessionError};
use hex_engine::PreparedRevision;
use hex_project::{Command, Project};
use hex_tuning::ProjectTuning;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuningLibraryOperation {
    List,
    Save { slot: usize, name: String },
    Load { slot: usize },
    Rename { slot: usize, name: String },
    Duplicate { slot: usize },
    Import { path: PathBuf },
    Export { path: PathBuf, name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TuningLibraryResult {
    pub operation_id: u64,
    pub slots: Vec<hex_storage::tuning_library::Slot>,
    pub selected_slot: Option<usize>,
    pub loaded_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TuningGestureKey {
    pub generation: u64,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TuningGestureRequest {
    Begin {
        key: TuningGestureKey,
    },
    Change {
        key: TuningGestureKey,
        sequence: u64,
        tuning: ProjectTuning,
    },
    Commit {
        key: TuningGestureKey,
        sequence: u64,
        tuning: ProjectTuning,
    },
    Cancel {
        key: TuningGestureKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuningGesturePhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct TuningGestureAck {
    pub key: TuningGestureKey,
    pub phase: TuningGesturePhase,
    pub sequence: u64,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TuningGestureSnapshot {
    pub key: TuningGestureKey,
    pub sequence: u64,
    pub tuning: ProjectTuning,
}

/// UI-side coalescer with a reserved terminal request. Queue-full retries
/// return the same in-flight value, and later changes cannot replace Commit or
/// Cancel after it has been reserved.
#[derive(Default)]
pub struct TuningGestureClient {
    next_token: u64,
    active: Option<TuningGestureSnapshot>,
    in_flight: Option<TuningGestureRequest>,
    submitted: bool,
    pending_begin: Option<TuningGestureRequest>,
    latest_change: Option<TuningGestureRequest>,
    terminal: Option<TuningGestureRequest>,
}

impl TuningGestureClient {
    pub fn begin(
        &mut self,
        generation: u64,
        initial: ProjectTuning,
    ) -> Result<TuningGestureKey, &'static str> {
        if self.active.is_some() {
            return Err("another tuning gesture is active");
        }
        self.next_token = self.next_token.saturating_add(1).max(1);
        let key = TuningGestureKey {
            generation,
            token: self.next_token,
        };
        self.active = Some(TuningGestureSnapshot {
            key,
            sequence: 0,
            tuning: initial,
        });
        self.pending_begin = Some(TuningGestureRequest::Begin { key });
        Ok(key)
    }

    pub fn change(&mut self, tuning: ProjectTuning) -> Result<u64, &'static str> {
        let (key, sequence) = {
            let active = self.mutable_active()?;
            active.sequence = active.sequence.saturating_add(1).max(1);
            active.tuning = tuning;
            (active.key, active.sequence)
        };
        self.latest_change = Some(TuningGestureRequest::Change {
            key,
            sequence,
            tuning,
        });
        Ok(sequence)
    }

    pub fn finish(&mut self, tuning: ProjectTuning) -> Result<u64, &'static str> {
        let (key, sequence) = {
            let active = self.mutable_active()?;
            active.sequence = active.sequence.saturating_add(1).max(1);
            active.tuning = tuning;
            (active.key, active.sequence)
        };
        self.latest_change = None;
        self.terminal = Some(TuningGestureRequest::Commit {
            key,
            sequence,
            tuning,
        });
        Ok(sequence)
    }

    pub fn cancel(&mut self) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("tuning gesture is already finishing");
        }
        let active = self.active.ok_or("no tuning gesture is active")?;
        self.latest_change = None;
        self.terminal = Some(TuningGestureRequest::Cancel { key: active.key });
        Ok(())
    }

    fn mutable_active(&mut self) -> Result<&mut TuningGestureSnapshot, &'static str> {
        if self.has_pending_terminal() {
            return Err("tuning gesture is finishing");
        }
        self.active.as_mut().ok_or("no tuning gesture is active")
    }

    pub fn next_request(&mut self) -> Option<TuningGestureRequest> {
        if self.submitted {
            return None;
        }
        if let Some(request) = self.in_flight {
            return Some(request);
        }
        let request = self
            .pending_begin
            .take()
            .or_else(|| self.terminal.take())
            .or_else(|| self.latest_change.take())?;
        self.in_flight = Some(request);
        Some(request)
    }

    pub fn mark_submitted(&mut self) {
        if self.in_flight.is_some() {
            self.submitted = true;
        }
    }

    pub fn acknowledge(&mut self, ack: &TuningGestureAck) -> bool {
        let Some(request) = self.in_flight else {
            return false;
        };
        if request.key() != ack.key
            || request.phase() != ack.phase
            || request.sequence() != ack.sequence
        {
            return false;
        }
        self.in_flight = None;
        self.submitted = false;
        if (!ack.accepted && ack.phase == TuningGesturePhase::Begin)
            || matches!(
                ack.phase,
                TuningGesturePhase::Commit | TuningGesturePhase::Cancel
            )
        {
            self.clear();
        }
        true
    }

    fn clear(&mut self) {
        self.active = None;
        self.pending_begin = None;
        self.latest_change = None;
        self.terminal = None;
        self.in_flight = None;
        self.submitted = false;
    }

    pub fn reset(&mut self) {
        self.clear();
    }

    pub fn snapshot(&self) -> Option<TuningGestureSnapshot> {
        self.active
    }

    /// Includes submitted requests until acknowledgement. Later project
    /// requests must not overtake a locally coalesced change or terminal.
    pub fn has_pending_requests(&self) -> bool {
        self.pending_begin.is_some()
            || self.latest_change.is_some()
            || self.terminal.is_some()
            || self.in_flight.is_some()
    }

    pub fn has_pending_terminal(&self) -> bool {
        self.terminal.is_some()
            || self.in_flight.is_some_and(|request| {
                matches!(
                    request.phase(),
                    TuningGesturePhase::Commit | TuningGesturePhase::Cancel
                )
            })
    }
}

impl TuningGestureRequest {
    pub fn key(self) -> TuningGestureKey {
        match self {
            Self::Begin { key }
            | Self::Change { key, .. }
            | Self::Commit { key, .. }
            | Self::Cancel { key } => key,
        }
    }

    pub fn phase(self) -> TuningGesturePhase {
        match self {
            Self::Begin { .. } => TuningGesturePhase::Begin,
            Self::Change { .. } => TuningGesturePhase::Change,
            Self::Commit { .. } => TuningGesturePhase::Commit,
            Self::Cancel { .. } => TuningGesturePhase::Cancel,
        }
    }

    pub fn sequence(self) -> u64 {
        match self {
            Self::Change { sequence, .. } | Self::Commit { sequence, .. } => sequence,
            Self::Begin { .. } | Self::Cancel { .. } => 0,
        }
    }
}

pub(crate) struct TuningGestureOutcome {
    pub ack: TuningGestureAck,
    pub playback: Option<Arc<PreparedRevision>>,
    pub preview_project: Option<Arc<Project>>,
    pub canonical_changed: bool,
}

#[derive(Default)]
pub(crate) struct TuningGestureMachine {
    active: Option<TuningGestureSnapshot>,
    initial: Option<ProjectTuning>,
    last_key: Option<TuningGestureKey>,
}

impl TuningGestureMachine {
    pub fn snapshot(&self) -> Option<TuningGestureSnapshot> {
        self.active
    }

    pub fn reset(&mut self) {
        self.active = None;
        self.initial = None;
        self.last_key = None;
    }

    #[cfg(test)]
    pub fn handle(
        &mut self,
        session: &mut Session,
        request: TuningGestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> TuningGestureOutcome {
        self.handle_with(session, request, previous, generation, |_, _, _| Ok(()))
    }

    pub fn handle_with(
        &mut self,
        session: &mut Session,
        request: TuningGestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        mut finish: impl FnMut(&Project, &Project, &Arc<PreparedRevision>) -> Result<(), SessionError>,
    ) -> TuningGestureOutcome {
        let mut outcome = TuningGestureOutcome {
            ack: TuningGestureAck {
                key: request.key(),
                phase: request.phase(),
                sequence: request.sequence(),
                accepted: false,
                reason: None,
            },
            playback: None,
            preview_project: None,
            canonical_changed: false,
        };
        if request.key().generation != generation || request.key().token == 0 {
            outcome.ack.reason = Some("tuning gesture belongs to an old project".into());
            return outcome;
        }
        let result = self.apply(session, request, previous, &mut outcome, &mut finish);
        match result {
            Ok(()) => outcome.ack.accepted = true,
            Err(error) => {
                outcome.ack.reason = Some(error.to_string().to_lowercase());
                if request.phase() == TuningGesturePhase::Commit
                    && request.sequence() != 0
                    && self.active.is_some_and(|active| {
                        active.key == request.key() && request.sequence() >= active.sequence
                    })
                {
                    self.active = None;
                    self.initial = None;
                    outcome.playback = previous.cloned();
                }
            }
        }
        outcome
    }

    fn apply(
        &mut self,
        session: &mut Session,
        request: TuningGestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        outcome: &mut TuningGestureOutcome,
        finish: &mut impl FnMut(&Project, &Project, &Arc<PreparedRevision>) -> Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        let key = request.key();
        let generation = key.generation;
        match request {
            TuningGestureRequest::Begin { .. } => {
                if self.active.is_some()
                    || self.last_key.is_some_and(|last| {
                        last.generation == generation && key.token <= last.token
                    })
                {
                    return Err(SessionError::Invalid("tuning gesture is active or stale"));
                }
                let tuning = session.project().tuning();
                self.active = Some(TuningGestureSnapshot {
                    key,
                    sequence: 0,
                    tuning,
                });
                self.initial = Some(tuning);
                self.last_key = Some(key);
            }
            TuningGestureRequest::Change {
                sequence, tuning, ..
            }
            | TuningGestureRequest::Commit {
                sequence, tuning, ..
            } => {
                let active = self.checked_active(session, key)?;
                let terminal = request.phase() == TuningGesturePhase::Commit;
                if sequence == 0
                    || sequence < active.sequence
                    || (!terminal && sequence == active.sequence)
                {
                    return Err(SessionError::Invalid("tuning gesture update is stale"));
                }
                if terminal {
                    let before = session.project().clone();
                    let (_, (prepared, ())) = session.command_prepared_revision_with(
                        Command::SetTuning(tuning),
                        previous.map(Arc::as_ref),
                        |source, target, prepared| finish(source, target, prepared),
                    )?;
                    outcome.canonical_changed = session.project() != &before;
                    outcome.playback = Some(prepared);
                    self.active = None;
                    self.initial = None;
                } else {
                    let (project, prepared) =
                        session.preview_tuning(tuning, previous.map(Arc::as_ref))?;
                    finish(session.project(), &project, &prepared)?;
                    if let Some(active) = self.active.as_mut() {
                        active.sequence = sequence;
                        active.tuning = tuning;
                    }
                    outcome.preview_project = Some(project);
                    outcome.playback = Some(prepared);
                }
            }
            TuningGestureRequest::Cancel { .. } => {
                self.checked_active(session, key)?;
                if let Some(prepared) = previous {
                    finish(session.project(), session.project(), prepared)?;
                }
                self.active = None;
                self.initial = None;
                outcome.playback = previous.cloned();
            }
        }
        Ok(())
    }

    fn checked_active(
        &self,
        session: &Session,
        key: TuningGestureKey,
    ) -> Result<TuningGestureSnapshot, SessionError> {
        let active = self
            .active
            .filter(|active| active.key == key)
            .ok_or(SessionError::Invalid("tuning gesture is no longer active"))?;
        let Some(initial) = self.initial else {
            return Err(SessionError::Invalid(
                "tuning gesture lost its initial value",
            ));
        };
        if session.project().tuning() != initial {
            return Err(SessionError::Invalid(
                "project tuning changed during gesture",
            ));
        }
        Ok(active)
    }

    /// Rebuild the transient tuning overlay after an unrelated canonical edit.
    pub fn rebase(
        &mut self,
        session: &Session,
        previous: Option<&Arc<PreparedRevision>>,
    ) -> Option<TuningGestureOutcome> {
        let active = self.active?;
        match self
            .checked_active(session, active.key)
            .and_then(|_| session.preview_tuning(active.tuning, previous.map(Arc::as_ref)))
        {
            Ok((project, prepared)) => Some(TuningGestureOutcome {
                ack: TuningGestureAck {
                    key: active.key,
                    phase: TuningGesturePhase::Change,
                    sequence: active.sequence,
                    accepted: true,
                    reason: None,
                },
                playback: Some(prepared),
                preview_project: Some(project),
                canonical_changed: false,
            }),
            Err(error) => {
                self.reset();
                Some(TuningGestureOutcome {
                    ack: TuningGestureAck {
                        key: active.key,
                        phase: TuningGesturePhase::Cancel,
                        sequence: active.sequence,
                        accepted: false,
                        reason: Some(error.to_string().to_lowercase()),
                    },
                    playback: previous.cloned(),
                    preview_project: None,
                    canonical_changed: false,
                })
            }
        }
    }

    pub fn finish_with(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        finish: impl FnMut(&Project, &Project, &Arc<PreparedRevision>) -> Result<(), SessionError>,
    ) -> Option<TuningGestureOutcome> {
        let active = self.active?;
        Some(self.handle_with(
            session,
            TuningGestureRequest::Commit {
                key: active.key,
                sequence: active.sequence.max(1),
                tuning: active.tuning,
            },
            previous,
            generation,
            finish,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_disabled_tuning() -> ProjectTuning {
        ProjectTuning::new(
            false,
            hex_tuning::Scale::default(),
            hex_tuning::Audition::new(hex_tuning::Waveform::Square, 0.3, 12.0, 700.0).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn client_coalesces_updates_and_reserves_terminal() {
        let mut client = TuningGestureClient::default();
        let key = client.begin(4, ProjectTuning::default()).unwrap();
        let begin = client.next_request().unwrap();
        assert_eq!(client.next_request(), Some(begin));
        let stale = TuningGestureAck {
            key: TuningGestureKey {
                generation: 4,
                token: key.token + 1,
            },
            phase: TuningGesturePhase::Begin,
            sequence: 0,
            accepted: true,
            reason: None,
        };
        assert!(!client.acknowledge(&stale));
        assert_eq!(client.next_request(), Some(begin));
        client.mark_submitted();
        let edited = ProjectTuning::default().with_enabled(false);
        client.change(edited).unwrap();
        client.finish(edited).unwrap();
        assert!(client.change(ProjectTuning::default()).is_err());
        let ack = |request: TuningGestureRequest| TuningGestureAck {
            key: request.key(),
            phase: request.phase(),
            sequence: request.sequence(),
            accepted: true,
            reason: None,
        };
        assert!(client.acknowledge(&ack(begin)));
        let terminal = client.next_request().unwrap();
        assert!(
            matches!(terminal, TuningGestureRequest::Commit { key: k, sequence: 2, .. } if k == key)
        );
        assert_eq!(client.next_request(), Some(terminal));
    }

    #[test]
    fn tuning_machine_preview_is_noncanonical_commit_is_one_undo_step_and_cancel_restores() {
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let previous = session.compile_prepared_revision(None).unwrap();
        let key = TuningGestureKey {
            generation: 2,
            token: 1,
        };
        let mut machine = TuningGestureMachine::default();
        let begin = machine.handle(
            &mut session,
            TuningGestureRequest::Begin { key },
            Some(&previous),
            2,
        );
        assert!(begin.ack.accepted);
        let changed = custom_disabled_tuning();
        let preview = machine.handle(
            &mut session,
            TuningGestureRequest::Change {
                key,
                sequence: 1,
                tuning: changed,
            },
            Some(&previous),
            2,
        );
        assert!(preview.ack.accepted);
        assert_eq!(session.project().tuning(), ProjectTuning::default());
        assert!(preview.preview_project.is_some());
        let committed = machine.handle(
            &mut session,
            TuningGestureRequest::Commit {
                key,
                sequence: 2,
                tuning: changed,
            },
            Some(&previous),
            2,
        );
        assert!(committed.ack.accepted);
        assert!(committed.canonical_changed);
        assert!(session.undo());
        assert_eq!(session.project().tuning(), ProjectTuning::default());
    }

    #[test]
    fn stale_and_wrong_generation_requests_do_not_retire_current_overlay() {
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let previous = session.compile_prepared_revision(None).unwrap();
        let key = TuningGestureKey {
            generation: 8,
            token: 1,
        };
        let mut machine = TuningGestureMachine::default();
        assert!(
            machine
                .handle(
                    &mut session,
                    TuningGestureRequest::Begin { key },
                    Some(&previous),
                    8
                )
                .ack
                .accepted
        );
        let stale = machine.handle(
            &mut session,
            TuningGestureRequest::Change {
                key,
                sequence: 1,
                tuning: ProjectTuning::default(),
            },
            Some(&previous),
            7,
        );
        assert!(!stale.ack.accepted);
        assert!(machine.snapshot().is_some());
        let bad_sequence = machine.handle(
            &mut session,
            TuningGestureRequest::Change {
                key,
                sequence: 0,
                tuning: ProjectTuning::default(),
            },
            Some(&previous),
            8,
        );
        assert!(!bad_sequence.ack.accepted);
        assert!(machine.snapshot().is_some());
        let tuning = custom_disabled_tuning();
        let preview = machine.handle(
            &mut session,
            TuningGestureRequest::Change {
                key,
                sequence: 1,
                tuning,
            },
            Some(&previous),
            8,
        );
        assert!(preview.ack.accepted);
        assert_ne!(
            preview.preview_project.as_ref().unwrap().tuning(),
            session.project().tuning()
        );
        let cancel = machine.handle(
            &mut session,
            TuningGestureRequest::Cancel { key },
            Some(&previous),
            8,
        );
        assert!(cancel.ack.accepted);
        assert_eq!(session.project().tuning(), ProjectTuning::default());
        assert!(
            cancel
                .playback
                .as_ref()
                .is_some_and(|restored| Arc::ptr_eq(restored, &previous))
        );
    }

    #[test]
    fn input_preparation_refusal_preserves_gesture_history_transaction() {
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let previous = session.compile_prepared_revision(None).unwrap();
        let original = session.project().clone();
        let key = TuningGestureKey {
            generation: 3,
            token: 1,
        };
        let mut machine = TuningGestureMachine::default();
        assert!(
            machine
                .handle(
                    &mut session,
                    TuningGestureRequest::Begin { key },
                    Some(&previous),
                    3,
                )
                .ack
                .accepted
        );
        let enabled = custom_disabled_tuning().with_enabled(true);
        let outcome = machine.handle_with(
            &mut session,
            TuningGestureRequest::Commit {
                key,
                sequence: 1,
                tuning: enabled,
            },
            Some(&previous),
            3,
            |_, target, _| {
                assert!(target.tuning().enabled());
                Err(SessionError::Invalid("input preparation refused"))
            },
        );
        assert!(!outcome.ack.accepted);
        assert_eq!(session.project(), &original);
        assert!(!session.can_undo());
        assert!(machine.snapshot().is_none());
    }
}
