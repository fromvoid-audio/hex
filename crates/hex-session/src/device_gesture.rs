//! One worker-owned, typed native parameter overlay. No callback ownership.
use crate::{Session, SessionError};
use hex_engine::PreparedRevision;
use hex_project::{DeviceId, Project};
use std::sync::Arc;

pub use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

/// Tokens are monotonic within a project-open generation; zero is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureKey {
    pub generation: u64,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureTarget {
    pub device: DeviceId,
    pub parameter: ParameterId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureRequest {
    Begin {
        key: GestureKey,
        target: GestureTarget,
    },
    Change {
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
    },
    Commit {
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
    },
    Cancel {
        key: GestureKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GesturePhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct GestureAck {
    pub key: GestureKey,
    pub phase: GesturePhase,
    pub sequence: u64,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureSnapshot {
    pub key: GestureKey,
    pub target: GestureTarget,
    pub sequence: u64,
    pub value: ParameterValue,
}

/// UI-side coalescer. It owns one in-flight request, one replaceable transient,
/// and a reserved terminal request which cannot be displaced by more changes.
#[derive(Default)]
pub struct GestureClient {
    next_token: u64,
    active: Option<GestureSnapshot>,
    in_flight: Option<GestureRequest>,
    submitted: bool,
    pending_begin: Option<GestureRequest>,
    latest_change: Option<GestureRequest>,
    terminal: Option<GestureRequest>,
}

impl GestureClient {
    pub fn begin(
        &mut self,
        generation: u64,
        target: GestureTarget,
        initial: ParameterValue,
    ) -> Result<GestureKey, &'static str> {
        if self.active.is_some() {
            return Err("another device gesture is active");
        }
        self.next_token = self.next_token.saturating_add(1).max(1);
        let key = GestureKey {
            generation,
            token: self.next_token,
        };
        self.active = Some(GestureSnapshot {
            key,
            target,
            sequence: 0,
            value: initial,
        });
        self.pending_begin = Some(GestureRequest::Begin { key, target });
        Ok(key)
    }

    pub fn change(&mut self, value: ParameterValue) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        self.latest_change = Some(GestureRequest::Change {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(snapshot.sequence)
    }

    pub fn finish(&mut self, value: ParameterValue) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        self.latest_change = None;
        self.terminal = Some(GestureRequest::Commit {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active else {
            return Err("no device gesture is active");
        };
        self.latest_change = None;
        self.terminal = Some(GestureRequest::Cancel { key: snapshot.key });
        Ok(())
    }

    /// Reserve the next protocol request before attempting the bounded service
    /// submit. On queue-full, call this again to retrieve the exact same request.
    pub fn next_request(&mut self) -> Option<GestureRequest> {
        if self.submitted {
            return None;
        }
        if let Some(in_flight) = self.in_flight {
            return Some(in_flight);
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

    pub fn reset(&mut self) {
        self.active = None;
        self.in_flight = None;
        self.submitted = false;
        self.pending_begin = None;
        self.latest_change = None;
        self.terminal = None;
    }

    /// A matching acknowledgment releases the protocol slot. Stale or unrelated
    /// responses leave the current gesture and its reserved terminal untouched.
    pub fn acknowledge(&mut self, ack: &GestureAck) -> bool {
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
        if !ack.accepted && ack.phase == GesturePhase::Begin {
            self.active = None;
            self.pending_begin = None;
            self.latest_change = None;
            self.terminal = None;
        } else if matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel) {
            self.active = None;
            self.latest_change = None;
            self.terminal = None;
            self.pending_begin = None;
        }
        true
    }

    pub fn snapshot(&self) -> Option<GestureSnapshot> {
        self.active
    }

    pub fn has_pending_terminal(&self) -> bool {
        self.terminal.is_some()
            || self.in_flight.is_some_and(|request| {
                matches!(request.phase(), GesturePhase::Commit | GesturePhase::Cancel)
            })
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;

    fn device() -> DeviceId {
        let mut session = Session::new(48_000, 4, 0).unwrap();
        let (track, _) = session
            .command_prepared_revision(
                hex_project::Command::AddTrack {
                    name: "test".into(),
                },
                None,
            )
            .unwrap();
        let track = match track {
            hex_project::Created::Track(track) => track,
            _ => unreachable!(),
        };
        let type_id = DeviceTypeId::new("hex.utility").unwrap();
        let state = hex_engine::default_native_device_state(type_id).unwrap();
        let (created, _) = session
            .command_prepared_revision(
                hex_project::Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                None,
            )
            .unwrap();
        match created {
            hex_project::Created::Device(device) => device,
            _ => unreachable!(),
        }
    }

    fn ack(request: GestureRequest, accepted: bool) -> GestureAck {
        GestureAck {
            key: request.key(),
            phase: request.phase(),
            sequence: request.sequence(),
            accepted,
            reason: None,
        }
    }

    #[test]
    fn coalesces_changes_and_preserves_final_value_until_terminal_ack() {
        let mut client = GestureClient::default();
        let target = GestureTarget {
            device: device(),
            parameter: ParameterId::new(1).unwrap(),
        };
        client.begin(0, target, ParameterValue::Float(1.0)).unwrap();
        let begin = client.next_request().unwrap();
        assert!(matches!(begin, GestureRequest::Begin { .. }));
        client.mark_submitted();
        assert_eq!(client.next_request(), None);

        client.change(ParameterValue::Float(2.0)).unwrap();
        client.change(ParameterValue::Float(3.0)).unwrap();
        client.finish(ParameterValue::Float(4.0)).unwrap();
        assert_eq!(
            client.change(ParameterValue::Float(5.0)),
            Err("device gesture is finishing")
        );
        assert!(client.acknowledge(&ack(begin, true)));
        let terminal = client.next_request().unwrap();
        assert_eq!(
            terminal,
            GestureRequest::Commit {
                key: begin.key(),
                sequence: 3,
                value: ParameterValue::Float(4.0),
            }
        );
        client.mark_submitted();
        assert_eq!(client.next_request(), None);
        assert!(client.acknowledge(&ack(terminal, true)));
        assert!(client.snapshot().is_none());
    }

    #[test]
    fn admitted_terminal_cannot_be_replaced_before_its_ack() {
        let mut client = GestureClient::default();
        let target = GestureTarget {
            device: device(),
            parameter: ParameterId::new(1).unwrap(),
        };
        client.begin(0, target, ParameterValue::Float(1.0)).unwrap();
        let begin = client.next_request().unwrap();
        client.mark_submitted();
        assert!(client.acknowledge(&ack(begin, true)));
        client.finish(ParameterValue::Float(2.0)).unwrap();
        let terminal = client.next_request().unwrap();
        client.mark_submitted();

        assert_eq!(
            client.finish(ParameterValue::Float(3.0)),
            Err("device gesture is already finishing")
        );
        assert_eq!(client.cancel(), Err("device gesture is already finishing"));
        assert_eq!(client.next_request(), None);
        assert_eq!(
            terminal,
            GestureRequest::Commit {
                key: begin.key(),
                sequence: 1,
                value: ParameterValue::Float(2.0),
            }
        );
        assert!(client.acknowledge(&ack(terminal, true)));
        assert!(client.snapshot().is_none());
    }

    #[test]
    fn queue_full_retry_keeps_exact_reserved_request_and_ignores_stale_ack() {
        let mut client = GestureClient::default();
        let target = GestureTarget {
            device: device(),
            parameter: ParameterId::new(1).unwrap(),
        };
        client.begin(2, target, ParameterValue::Float(1.0)).unwrap();
        let begin = client.next_request().unwrap();
        assert_eq!(client.next_request(), Some(begin));
        let wrong = GestureAck {
            key: GestureKey {
                generation: 2,
                token: 99,
            },
            phase: GesturePhase::Begin,
            sequence: 0,
            accepted: true,
            reason: None,
        };
        assert!(!client.acknowledge(&wrong));
        assert_eq!(client.next_request(), Some(begin));
        client.mark_submitted();
        client.change(ParameterValue::Float(2.0)).unwrap();
        client.finish(ParameterValue::Float(3.0)).unwrap();
        assert!(client.acknowledge(&ack(begin, true)));
        let terminal = client.next_request().unwrap();
        assert!(matches!(
            terminal,
            GestureRequest::Commit {
                value: ParameterValue::Float(3.0),
                ..
            }
        ));
    }
}

pub(crate) struct GestureOutcome {
    pub ack: GestureAck,
    pub playback: Option<Arc<PreparedRevision>>,
    pub preview_project: Option<Arc<Project>>,
    pub canonical_changed: bool,
}

struct Active {
    snapshot: GestureSnapshot,
    type_id: DeviceTypeId,
}

#[derive(Default)]
pub(crate) struct GestureMachine {
    active: Option<Active>,
    last_key: Option<GestureKey>,
}

impl GestureRequest {
    pub fn key(self) -> GestureKey {
        match self {
            Self::Begin { key, .. }
            | Self::Change { key, .. }
            | Self::Commit { key, .. }
            | Self::Cancel { key } => key,
        }
    }
    pub fn phase(self) -> GesturePhase {
        match self {
            Self::Begin { .. } => GesturePhase::Begin,
            Self::Change { .. } => GesturePhase::Change,
            Self::Commit { .. } => GesturePhase::Commit,
            Self::Cancel { .. } => GesturePhase::Cancel,
        }
    }
    pub fn sequence(self) -> u64 {
        match self {
            Self::Change { sequence, .. } | Self::Commit { sequence, .. } => sequence,
            _ => 0,
        }
    }
}

impl GestureMachine {
    pub fn snapshot(&self) -> Option<GestureSnapshot> {
        self.active.as_ref().map(|active| active.snapshot)
    }

    pub fn reset(&mut self) {
        self.active = None;
        self.last_key = None;
    }

    pub fn handle(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> GestureOutcome {
        let mut outcome = GestureOutcome {
            ack: GestureAck {
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
        let result = self.apply(session, request, previous, generation, &mut outcome);
        match result {
            Ok(()) => outcome.ack.accepted = true,
            Err(error) => {
                outcome.ack.reason = Some(error.to_string().to_lowercase());
                // A refused terminal still retires its own overlay and restores
                // canonical sound. Never retire another gesture for a stale key.
                if request.phase() == GesturePhase::Commit
                    && request.sequence() != 0
                    && self.snapshot().is_some_and(|active| {
                        active.key == request.key() && request.sequence() >= active.sequence
                    })
                {
                    self.active = None;
                    outcome.playback = previous.cloned();
                }
            }
        }
        outcome
    }

    fn apply(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        outcome: &mut GestureOutcome,
    ) -> Result<(), SessionError> {
        let key = request.key();
        if key.generation != generation || key.token == 0 {
            return Err(SessionError::Invalid(
                "device gesture belongs to an old project",
            ));
        }
        match request {
            GestureRequest::Begin { target, .. } => {
                if self.active.is_some() {
                    return Err(SessionError::Invalid("another device gesture is active"));
                }
                if self
                    .last_key
                    .is_some_and(|last| last.generation == generation && key.token <= last.token)
                {
                    return Err(SessionError::Invalid("device gesture token is stale"));
                }
                let device = session
                    .project()
                    .device(target.device)
                    .ok_or(SessionError::Invalid("device no longer exists"))?;
                let value = hex_engine::native_device_parameter(device.state(), target.parameter)?;
                self.active = Some(Active {
                    snapshot: GestureSnapshot {
                        key,
                        target,
                        sequence: 0,
                        value,
                    },
                    type_id: device.state().type_id(),
                });
                self.last_key = Some(key);
            }
            GestureRequest::Change {
                sequence, value, ..
            } => {
                let active = self.checked_active(session, key)?;
                if sequence == 0 || sequence <= active.sequence {
                    return Err(SessionError::Invalid("device gesture update is stale"));
                }
                let (project, prepared) = session.preview_device_parameter(
                    active.target.device,
                    active.target.parameter,
                    value,
                    previous.map(Arc::as_ref),
                )?;
                if let Some(current) = self.active.as_mut() {
                    current.snapshot.sequence = sequence;
                    current.snapshot.value = value;
                }
                outcome.preview_project = Some(project);
                outcome.playback = Some(prepared);
            }
            GestureRequest::Commit {
                sequence, value, ..
            } => {
                let active = self.checked_active(session, key)?;
                if sequence == 0 || sequence < active.sequence {
                    return Err(SessionError::Invalid("device gesture commit is stale"));
                }
                let before = session.project().clone();
                let prepared = session.commit_device_parameter(
                    active.target.device,
                    active.target.parameter,
                    value,
                    previous.map(Arc::as_ref),
                )?;
                outcome.canonical_changed = session.project() != &before;
                outcome.playback = Some(prepared);
                self.active = None;
            }
            GestureRequest::Cancel { .. } => {
                self.checked_key(key)?;
                self.active = None;
                outcome.playback = previous.cloned();
            }
        }
        Ok(())
    }

    fn checked_key(&self, key: GestureKey) -> Result<GestureSnapshot, SessionError> {
        self.snapshot()
            .filter(|active| active.key == key)
            .ok_or(SessionError::Invalid("device gesture is no longer active"))
    }
    fn checked_active(
        &self,
        session: &Session,
        key: GestureKey,
    ) -> Result<GestureSnapshot, SessionError> {
        let snapshot = self.checked_key(key)?;
        let type_id = self
            .active
            .as_ref()
            .map(|active| active.type_id)
            .ok_or(SessionError::Invalid("device gesture is no longer active"))?;
        let device = session
            .project()
            .device(snapshot.target.device)
            .filter(|device| device.state().type_id() == type_id)
            .ok_or(SessionError::Invalid(
                "device gesture target was removed or replaced",
            ))?;
        hex_engine::native_device_parameter(device.state(), snapshot.target.parameter)?;
        Ok(snapshot)
    }

    /// Rebase a still-active overlay after an independent canonical edit. If
    /// that edit removes its target or makes the overlay invalid, retire only
    /// the overlay; the successful canonical edit and history remain intact.
    pub fn rebase(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        let result = self.checked_active(session, active.key).and_then(|_| {
            session.preview_device_parameter(
                active.target.device,
                active.target.parameter,
                active.value,
                previous.map(Arc::as_ref),
            )
        });
        match result {
            Ok((project, prepared)) => Some(GestureOutcome {
                ack: GestureAck {
                    key: active.key,
                    phase: GesturePhase::Change,
                    sequence: active.sequence,
                    accepted: true,
                    reason: None,
                },
                playback: Some(prepared),
                preview_project: Some(project),
                canonical_changed: false,
            }),
            Err(error) => {
                let mut result = self.handle(
                    session,
                    GestureRequest::Cancel { key: active.key },
                    previous,
                    generation,
                );
                result.ack.reason = Some(error.to_string().to_lowercase());
                Some(result)
            }
        }
    }

    pub fn finish(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        Some(self.handle(
            session,
            GestureRequest::Commit {
                key: active.key,
                sequence: active.sequence.max(1),
                value: active.value,
            },
            previous,
            generation,
        ))
    }
}
