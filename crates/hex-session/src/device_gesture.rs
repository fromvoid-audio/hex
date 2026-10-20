//! One worker-owned, typed native parameter overlay. No callback ownership.
use crate::{Session, SessionError};
use hex_engine::{NativeDeviceClass, PreparedRevision};
use hex_project::{DeviceId, Project, ReturnId, TrackId};
use std::sync::Arc;
#[cfg(test)]
mod snapshot_tests;

pub use hex_lattice_contract::{
    DeviceParameterSnapshot, DeviceTypeId, ParameterId, ParameterValue,
};

/// Tokens are monotonic within a project-open generation; zero is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureKey {
    pub generation: u64,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureTarget {
    TrackGain {
        track: TrackId,
    },
    TrackBalance {
        track: TrackId,
    },
    ReturnGain {
        return_track: ReturnId,
    },
    DeviceParameter {
        device: DeviceId,
        parameter: ParameterId,
    },
    DeviceParameterPair {
        device: DeviceId,
        first: ParameterId,
        second: ParameterId,
    },
    DeviceParameterSnapshot {
        device: DeviceId,
    },
    ReturnSend {
        track: TrackId,
        return_track: ReturnId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterPairValue {
    pub first: ParameterValue,
    pub second: ParameterValue,
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
    ChangePair {
        key: GestureKey,
        sequence: u64,
        value: ParameterPairValue,
    },
    Commit {
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
    },
    CommitPair {
        key: GestureKey,
        sequence: u64,
        value: ParameterPairValue,
    },
    BeginSnapshot {
        key: GestureKey,
        device: DeviceId,
        snapshot: DeviceParameterSnapshot,
    },
    ChangeSnapshot {
        key: GestureKey,
        sequence: u64,
        snapshot: DeviceParameterSnapshot,
    },
    CommitSnapshot {
        key: GestureKey,
        sequence: u64,
        snapshot: DeviceParameterSnapshot,
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
    pub secondary_value: Option<ParameterValue>,
    pub parameters: Option<DeviceParameterSnapshot>,
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
            secondary_value: None,
            parameters: None,
        });
        self.pending_begin = Some(GestureRequest::Begin { key, target });
        Ok(key)
    }

    pub fn begin_pair(
        &mut self,
        generation: u64,
        device: DeviceId,
        first: ParameterId,
        second: ParameterId,
        initial: ParameterPairValue,
    ) -> Result<GestureKey, &'static str> {
        if self.active.is_some() {
            return Err("another device gesture is active");
        }
        if first == second {
            return Err("paired device gesture needs distinct parameters");
        }
        self.next_token = self.next_token.saturating_add(1).max(1);
        let key = GestureKey {
            generation,
            token: self.next_token,
        };
        let target = GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        };
        self.active = Some(GestureSnapshot {
            key,
            target,
            sequence: 0,
            value: initial.first,
            secondary_value: Some(initial.second),
            parameters: None,
        });
        self.pending_begin = Some(GestureRequest::Begin { key, target });
        Ok(key)
    }

    pub fn begin_snapshot(
        &mut self,
        generation: u64,
        device: DeviceId,
        initial: DeviceParameterSnapshot,
    ) -> Result<GestureKey, &'static str> {
        let key = self.begin(
            generation,
            GestureTarget::DeviceParameterSnapshot { device },
            ParameterValue::Float(0.0),
        )?;
        self.active.as_mut().unwrap().parameters = Some(initial);
        self.pending_begin = Some(GestureRequest::BeginSnapshot {
            key,
            device,
            snapshot: initial,
        });
        Ok(key)
    }

    pub fn change_snapshot(&mut self, value: DeviceParameterSnapshot) -> Result<u64, &'static str> {
        self.update_snapshot(value, false)
    }

    pub fn finish_snapshot(&mut self, value: DeviceParameterSnapshot) -> Result<(), &'static str> {
        self.update_snapshot(value, true).map(|_| ())
    }

    fn update_snapshot(
        &mut self,
        value: DeviceParameterSnapshot,
        terminal: bool,
    ) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let active = self.active.as_mut().ok_or("no device gesture is active")?;
        if !active
            .parameters
            .is_some_and(|initial| initial.same_parameters(&value))
        {
            return Err("snapshot gesture parameters changed");
        }
        active.sequence = active.sequence.saturating_add(1).max(1);
        active.parameters = Some(value);
        let request = if terminal {
            GestureRequest::CommitSnapshot {
                key: active.key,
                sequence: active.sequence,
                snapshot: value,
            }
        } else {
            GestureRequest::ChangeSnapshot {
                key: active.key,
                sequence: active.sequence,
                snapshot: value,
            }
        };
        if terminal {
            self.latest_change = None;
            self.terminal = Some(request);
        } else {
            self.latest_change = Some(request);
        }
        Ok(active.sequence)
    }

    pub fn change(&mut self, value: ParameterValue) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if matches!(
            snapshot.target,
            GestureTarget::DeviceParameterPair { .. }
                | GestureTarget::DeviceParameterSnapshot { .. }
        ) {
            return Err("active gesture requires paired values");
        }
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        snapshot.secondary_value = None;
        self.latest_change = Some(GestureRequest::Change {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(snapshot.sequence)
    }

    pub fn change_pair(&mut self, value: ParameterPairValue) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if !matches!(snapshot.target, GestureTarget::DeviceParameterPair { .. }) {
            return Err("active gesture does not accept paired values");
        }
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value.first;
        snapshot.secondary_value = Some(value.second);
        self.latest_change = Some(GestureRequest::ChangePair {
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
        if matches!(
            snapshot.target,
            GestureTarget::DeviceParameterPair { .. }
                | GestureTarget::DeviceParameterSnapshot { .. }
        ) {
            return Err("active gesture requires paired values");
        }
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value;
        snapshot.secondary_value = None;
        self.latest_change = None;
        self.terminal = Some(GestureRequest::Commit {
            key: snapshot.key,
            sequence: snapshot.sequence,
            value,
        });
        Ok(())
    }

    pub fn finish_pair(&mut self, value: ParameterPairValue) -> Result<(), &'static str> {
        if self.has_pending_terminal() {
            return Err("device gesture is already finishing");
        }
        let Some(snapshot) = self.active.as_mut() else {
            return Err("no device gesture is active");
        };
        if !matches!(snapshot.target, GestureTarget::DeviceParameterPair { .. }) {
            return Err("active gesture does not accept paired values");
        }
        snapshot.sequence = snapshot.sequence.saturating_add(1).max(1);
        snapshot.value = value.first;
        snapshot.secondary_value = Some(value.second);
        self.latest_change = None;
        self.terminal = Some(GestureRequest::CommitPair {
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

    /// Finalize the currently authored overlay without dropping compound values.
    pub fn finish_current(&mut self) -> Result<(), &'static str> {
        let active = self.active.ok_or("no device gesture is active")?;
        if let Some(parameters) = active.parameters {
            self.finish_snapshot(parameters)
        } else if let Some(second) = active.secondary_value {
            self.finish_pair(ParameterPairValue {
                first: active.value,
                second,
            })
        } else {
            self.finish(active.value)
        }
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
        let target = GestureTarget::DeviceParameter {
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
        let target = GestureTarget::DeviceParameter {
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
        let target = GestureTarget::DeviceParameter {
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

    #[test]
    fn paired_client_coalesces_two_values_as_one_terminal_command() {
        let mut client = GestureClient::default();
        let device = device();
        let target_first = ParameterId::new(10).unwrap();
        let target_second = ParameterId::new(11).unwrap();
        let initial = ParameterPairValue {
            first: ParameterValue::Float(1_000.0),
            second: ParameterValue::Float(0.0),
        };
        client
            .begin_pair(3, device, target_first, target_second, initial)
            .unwrap();
        let begin = client.next_request().unwrap();
        client.mark_submitted();
        client
            .change_pair(ParameterPairValue {
                first: ParameterValue::Float(2_000.0),
                second: ParameterValue::Float(2.0),
            })
            .unwrap();
        client
            .change_pair(ParameterPairValue {
                first: ParameterValue::Float(3_000.0),
                second: ParameterValue::Float(3.0),
            })
            .unwrap();
        client
            .finish_pair(ParameterPairValue {
                first: ParameterValue::Float(4_000.0),
                second: ParameterValue::Float(4.0),
            })
            .unwrap();
        assert_eq!(
            client.change_pair(initial),
            Err("device gesture is finishing")
        );
        assert!(client.acknowledge(&ack(begin, true)));
        let terminal = client.next_request().unwrap();
        assert_eq!(
            terminal,
            GestureRequest::CommitPair {
                key: begin.key(),
                sequence: 3,
                value: ParameterPairValue {
                    first: ParameterValue::Float(4_000.0),
                    second: ParameterValue::Float(4.0),
                },
            }
        );
        assert_eq!(client.next_request(), Some(terminal));
        client.mark_submitted();
        assert!(client.acknowledge(&ack(terminal, true)));
        assert!(client.snapshot().is_none());
    }

    #[test]
    fn paired_commit_is_atomic_and_undoes_as_one_state_change() {
        let mut session = Session::new(48_000, 4, 0).unwrap();
        let (created_track, _) = session
            .command_prepared_revision(
                hex_project::Command::AddTrack {
                    name: "test".into(),
                },
                None,
            )
            .unwrap();
        let hex_project::Created::Track(track) = created_track else {
            panic!("track command returned an unexpected object");
        };
        let eq8_type = DeviceTypeId::new("hex.eq8").unwrap();
        let state = hex_engine::default_native_device_state(eq8_type).unwrap();
        let (created_device, prepared) = session
            .command_prepared_revision(
                hex_project::Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                None,
            )
            .unwrap();
        let hex_project::Created::Device(device) = created_device else {
            panic!("device command returned an unexpected object");
        };
        let original = session.project().device(device).unwrap().state().clone();
        let first = ParameterId::new(18).unwrap();
        let second = ParameterId::new(19).unwrap();
        assert!(
            session
                .preview_device_parameter_pair(
                    device,
                    (first, ParameterValue::Float(2_000.0)),
                    (first, ParameterValue::Float(2.0)),
                    Some(&prepared),
                )
                .is_err()
        );
        assert!(
            session
                .commit_device_parameter_pair(
                    device,
                    (first, ParameterValue::Float(2_000.0)),
                    (first, ParameterValue::Float(2.0)),
                    Some(&prepared),
                )
                .is_err()
        );
        assert_eq!(session.project().device(device).unwrap().state(), &original);
        let key = GestureKey {
            generation: 1,
            token: 1,
        };
        let mut machine = GestureMachine::default();
        let begun = machine.handle(
            &mut session,
            GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameterPair {
                    device,
                    first,
                    second,
                },
            },
            Some(&prepared),
            1,
        );
        assert!(begun.ack.accepted);
        let changed = machine.handle(
            &mut session,
            GestureRequest::ChangePair {
                key,
                sequence: 1,
                value: ParameterPairValue {
                    first: ParameterValue::Float(1_500.0),
                    second: ParameterValue::Float(6.0),
                },
            },
            Some(&prepared),
            1,
        );
        assert!(changed.ack.accepted);
        let preview_state = changed
            .preview_project
            .as_ref()
            .unwrap()
            .device(device)
            .unwrap()
            .state();
        assert_eq!(
            hex_engine::native_device_parameter(preview_state, first).unwrap(),
            ParameterValue::Float(1_500.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(preview_state, second).unwrap(),
            ParameterValue::Float(6.0)
        );
        assert_eq!(session.project().device(device).unwrap().state(), &original);

        let committed = machine.handle(
            &mut session,
            GestureRequest::CommitPair {
                key,
                sequence: 2,
                value: ParameterPairValue {
                    first: ParameterValue::Float(1_500.0),
                    second: ParameterValue::Float(6.0),
                },
            },
            Some(&prepared),
            1,
        );
        assert!(committed.ack.accepted);
        assert!(committed.canonical_changed);
        let committed_state = session.project().device(device).unwrap().state();
        assert_eq!(
            hex_engine::native_device_parameter(committed_state, first).unwrap(),
            ParameterValue::Float(1_500.0)
        );
        assert_eq!(
            hex_engine::native_device_parameter(committed_state, second).unwrap(),
            ParameterValue::Float(6.0)
        );
        assert!(session.undo());
        assert_eq!(session.project().device(device).unwrap().state(), &original);

        let before = session.project().clone();
        let result = session.commit_device_parameter_pair(
            device,
            (first, ParameterValue::Float(2_000.0)),
            (second, ParameterValue::Float(f32::NAN)),
            Some(&prepared),
        );
        assert!(result.is_err());
        assert_eq!(session.project(), &before);
        assert_eq!(session.project().device(device).unwrap().state(), &original);
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
    kind: ActiveKind,
}

#[derive(Clone, Copy)]
enum ActiveKind {
    TrackGain,
    TrackBalance,
    ReturnGain,
    Device {
        class: NativeDeviceClass,
        type_id: DeviceTypeId,
    },
    ReturnSend,
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
            | Self::ChangePair { key, .. }
            | Self::Commit { key, .. }
            | Self::CommitPair { key, .. }
            | Self::BeginSnapshot { key, .. }
            | Self::ChangeSnapshot { key, .. }
            | Self::CommitSnapshot { key, .. }
            | Self::Cancel { key } => key,
        }
    }
    pub fn phase(self) -> GesturePhase {
        match self {
            Self::Begin { .. } | Self::BeginSnapshot { .. } => GesturePhase::Begin,
            Self::Change { .. } => GesturePhase::Change,
            Self::ChangePair { .. } | Self::ChangeSnapshot { .. } => GesturePhase::Change,
            Self::Commit { .. } => GesturePhase::Commit,
            Self::CommitPair { .. } | Self::CommitSnapshot { .. } => GesturePhase::Commit,
            Self::Cancel { .. } => GesturePhase::Cancel,
        }
    }
    pub fn sequence(self) -> u64 {
        match self {
            Self::Change { sequence, .. }
            | Self::ChangePair { sequence, .. }
            | Self::Commit { sequence, .. }
            | Self::CommitPair { sequence, .. }
            | Self::ChangeSnapshot { sequence, .. }
            | Self::CommitSnapshot { sequence, .. } => sequence,
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
            GestureRequest::BeginSnapshot {
                device, snapshot, ..
            } => {
                if self.active.is_some()
                    || self.last_key.is_some_and(|last| {
                        last.generation == generation && key.token <= last.token
                    })
                {
                    return Err(SessionError::Invalid("snapshot gesture is active or stale"));
                }
                let class = session.native_device_class(device)?;
                let state = session
                    .project()
                    .device(device)
                    .ok_or(SessionError::Invalid("device no longer exists"))?
                    .state();
                // Read canonical values rather than trusting caller's begin values.
                let mut entries = [(ParameterId::from_static(1), ParameterValue::Float(0.0)); 64];
                for (index, &(id, _)) in snapshot.entries().iter().enumerate() {
                    entries[index] = (
                        id,
                        hex_engine::native_device_parameter_for_class(class, state, id)?,
                    );
                }
                let initial = DeviceParameterSnapshot::new(&entries[..snapshot.entries().len()])
                    .map_err(|_| SessionError::Invalid("invalid snapshot parameters"))?;
                // Validate caller values before accepting ownership, without publication.
                hex_engine::patch_native_device_snapshot_for_class(class, state, &snapshot)?;
                self.active = Some(Active {
                    snapshot: GestureSnapshot {
                        key,
                        target: GestureTarget::DeviceParameterSnapshot { device },
                        sequence: 0,
                        value: ParameterValue::Float(0.0),
                        secondary_value: None,
                        parameters: Some(initial),
                    },
                    kind: ActiveKind::Device {
                        class,
                        type_id: state.type_id(),
                    },
                });
                self.last_key = Some(key);
            }
            GestureRequest::ChangeSnapshot {
                sequence, snapshot, ..
            }
            | GestureRequest::CommitSnapshot {
                sequence, snapshot, ..
            } => {
                let active = self.checked_active(session, key)?;
                let terminal = request.phase() == GesturePhase::Commit;
                if sequence == 0
                    || sequence < active.sequence
                    || (!terminal && sequence == active.sequence)
                {
                    return Err(SessionError::Invalid("snapshot gesture update is stale"));
                }
                if !active
                    .parameters
                    .is_some_and(|value| value.same_parameters(&snapshot))
                {
                    return Err(SessionError::Invalid("snapshot gesture parameters changed"));
                }
                let GestureTarget::DeviceParameterSnapshot { device } = active.target else {
                    return Err(SessionError::Invalid("snapshot gesture target changed"));
                };
                if terminal {
                    let before = session.project().clone();
                    outcome.playback = Some(session.commit_device_snapshot(
                        device,
                        &snapshot,
                        previous.map(Arc::as_ref),
                    )?);
                    outcome.canonical_changed = session.project() != &before;
                    self.active = None;
                } else {
                    let (project, prepared) = session.preview_device_snapshot(
                        device,
                        &snapshot,
                        previous.map(Arc::as_ref),
                    )?;
                    let current = self.active.as_mut().unwrap();
                    current.snapshot.sequence = sequence;
                    current.snapshot.parameters = Some(snapshot);
                    outcome.preview_project = Some(project);
                    outcome.playback = Some(prepared);
                }
            }
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
                let (kind, value, secondary_value) = match target {
                    GestureTarget::DeviceParameterSnapshot { .. } => {
                        return Err(SessionError::Invalid(
                            "snapshot gesture requires initial parameters",
                        ));
                    }
                    GestureTarget::DeviceParameter { device, parameter } => {
                        let class = session.native_device_class(device)?;
                        let device_state = session
                            .project()
                            .device(device)
                            .ok_or(SessionError::Invalid("device no longer exists"))?;
                        let value = hex_engine::native_device_parameter_for_class(
                            class,
                            device_state.state(),
                            parameter,
                        )?;
                        (
                            ActiveKind::Device {
                                class,
                                type_id: device_state.state().type_id(),
                            },
                            value,
                            None,
                        )
                    }
                    GestureTarget::DeviceParameterPair {
                        device,
                        first,
                        second,
                    } => {
                        if first == second {
                            return Err(SessionError::Invalid(
                                "paired device gesture needs distinct parameters",
                            ));
                        }
                        let class = session.native_device_class(device)?;
                        let device_state = session
                            .project()
                            .device(device)
                            .ok_or(SessionError::Invalid("device no longer exists"))?;
                        let first_value = hex_engine::native_device_parameter_for_class(
                            class,
                            device_state.state(),
                            first,
                        )?;
                        let second_value = hex_engine::native_device_parameter_for_class(
                            class,
                            device_state.state(),
                            second,
                        )?;
                        (
                            ActiveKind::Device {
                                class,
                                type_id: device_state.state().type_id(),
                            },
                            first_value,
                            Some(second_value),
                        )
                    }
                    GestureTarget::TrackBalance { track } => (
                        ActiveKind::TrackBalance,
                        ParameterValue::Float(static_track_balance(session, track)?),
                        None,
                    ),
                    GestureTarget::ReturnGain { return_track } => (
                        ActiveKind::ReturnGain,
                        ParameterValue::Float(static_return_gain(session, return_track)?),
                        None,
                    ),
                    GestureTarget::TrackGain { track } => (
                        ActiveKind::TrackGain,
                        ParameterValue::Float(static_track_gain(session, track)?),
                        None,
                    ),
                    GestureTarget::ReturnSend {
                        track,
                        return_track,
                    } => {
                        let source = session
                            .project()
                            .track(track)
                            .ok_or(SessionError::Invalid("return send source no longer exists"))?;
                        if !session
                            .project()
                            .return_tracks()
                            .iter()
                            .any(|candidate| candidate.id() == return_track)
                        {
                            return Err(SessionError::Invalid(
                                "return send target no longer exists",
                            ));
                        }
                        let gain = source
                            .sends()
                            .iter()
                            .find(|send| send.return_track() == return_track)
                            .map_or(0.0, hex_project::SendLevel::gain);
                        (ActiveKind::ReturnSend, ParameterValue::Float(gain), None)
                    }
                };
                self.active = Some(Active {
                    snapshot: GestureSnapshot {
                        key,
                        target,
                        sequence: 0,
                        value,
                        secondary_value,
                        parameters: None,
                    },
                    kind,
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
                let (project, prepared) =
                    preview_target(session, active.target, value, previous.map(Arc::as_ref))?;
                if let Some(current) = self.active.as_mut() {
                    current.snapshot.sequence = sequence;
                    current.snapshot.value = value;
                }
                outcome.preview_project = Some(project);
                outcome.playback = Some(prepared);
            }
            GestureRequest::ChangePair {
                sequence, value, ..
            } => {
                let active = self.checked_active(session, key)?;
                if sequence == 0 || sequence <= active.sequence {
                    return Err(SessionError::Invalid("device gesture update is stale"));
                }
                let (project, prepared) =
                    preview_pair_target(session, active.target, value, previous.map(Arc::as_ref))?;
                if let Some(current) = self.active.as_mut() {
                    current.snapshot.sequence = sequence;
                    current.snapshot.value = value.first;
                    current.snapshot.secondary_value = Some(value.second);
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
                let prepared =
                    commit_target(session, active.target, value, previous.map(Arc::as_ref))?;
                outcome.canonical_changed = session.project() != &before;
                outcome.playback = Some(prepared);
                self.active = None;
            }
            GestureRequest::CommitPair {
                sequence, value, ..
            } => {
                let active = self.checked_active(session, key)?;
                if sequence == 0 || sequence < active.sequence {
                    return Err(SessionError::Invalid("device gesture commit is stale"));
                }
                let before = session.project().clone();
                let prepared =
                    commit_pair_target(session, active.target, value, previous.map(Arc::as_ref))?;
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
        let active = self
            .active
            .as_ref()
            .ok_or(SessionError::Invalid("device gesture is no longer active"))?;
        match (snapshot.target, active.kind) {
            (GestureTarget::TrackBalance { track }, ActiveKind::TrackBalance) => {
                static_track_balance(session, track)?;
            }
            (GestureTarget::ReturnGain { return_track }, ActiveKind::ReturnGain) => {
                static_return_gain(session, return_track)?;
            }
            (GestureTarget::TrackGain { track }, ActiveKind::TrackGain) => {
                static_track_gain(session, track)?;
            }
            (
                GestureTarget::DeviceParameterSnapshot { device },
                ActiveKind::Device { class, type_id },
            ) => {
                let state = session
                    .project()
                    .device(device)
                    .filter(|device| device.state().type_id() == type_id)
                    .ok_or(SessionError::Invalid(
                        "device gesture target was removed or replaced",
                    ))?
                    .state();
                if session.native_device_class(device)? != class {
                    return Err(SessionError::Invalid(
                        "device gesture target changed native class",
                    ));
                }
                for &(id, _) in snapshot
                    .parameters
                    .ok_or(SessionError::Invalid("missing snapshot parameters"))?
                    .entries()
                {
                    hex_engine::native_device_parameter_for_class(class, state, id)?;
                }
            }

            (
                GestureTarget::DeviceParameter { device, parameter },
                ActiveKind::Device { class, type_id },
            ) => {
                let device_state = session
                    .project()
                    .device(device)
                    .filter(|device| device.state().type_id() == type_id)
                    .ok_or(SessionError::Invalid(
                        "device gesture target was removed or replaced",
                    ))?;
                if session.native_device_class(device)? != class {
                    return Err(SessionError::Invalid(
                        "device gesture target changed native class",
                    ));
                }
                hex_engine::native_device_parameter_for_class(
                    class,
                    device_state.state(),
                    parameter,
                )?;
            }
            (
                GestureTarget::DeviceParameterPair {
                    device,
                    first,
                    second,
                },
                ActiveKind::Device { class, type_id },
            ) => {
                let device_state = session
                    .project()
                    .device(device)
                    .filter(|device| device.state().type_id() == type_id)
                    .ok_or(SessionError::Invalid(
                        "device gesture target was removed or replaced",
                    ))?;
                if first == second || session.native_device_class(device)? != class {
                    return Err(SessionError::Invalid(
                        "paired device gesture target changed",
                    ));
                }
                hex_engine::native_device_parameter_for_class(class, device_state.state(), first)?;
                hex_engine::native_device_parameter_for_class(class, device_state.state(), second)?;
            }
            (
                GestureTarget::ReturnSend {
                    track,
                    return_track,
                },
                ActiveKind::ReturnSend,
            ) => {
                if session.project().track(track).is_none() {
                    return Err(SessionError::Invalid("return send source was removed"));
                }
                if !session
                    .project()
                    .return_tracks()
                    .iter()
                    .any(|candidate| candidate.id() == return_track)
                {
                    return Err(SessionError::Invalid("return send target was removed"));
                }
            }
            _ => return Err(SessionError::Invalid("numeric gesture target changed kind")),
        }
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
            if let Some(parameters) = active.parameters {
                let GestureTarget::DeviceParameterSnapshot { device } = active.target else {
                    return Err(SessionError::Invalid("snapshot target changed"));
                };
                session.preview_device_snapshot(device, &parameters, previous.map(Arc::as_ref))
            } else if let Some(second) = active.secondary_value {
                preview_pair_target(
                    session,
                    active.target,
                    ParameterPairValue {
                        first: active.value,
                        second,
                    },
                    previous.map(Arc::as_ref),
                )
            } else {
                preview_target(
                    session,
                    active.target,
                    active.value,
                    previous.map(Arc::as_ref),
                )
            }
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
        let request = if let Some(snapshot) = active.parameters {
            GestureRequest::CommitSnapshot {
                key: active.key,
                sequence: active.sequence.max(1),
                snapshot,
            }
        } else if let Some(second) = active.secondary_value {
            GestureRequest::CommitPair {
                key: active.key,
                sequence: active.sequence.max(1),
                value: ParameterPairValue {
                    first: active.value,
                    second,
                },
            }
        } else {
            GestureRequest::Commit {
                key: active.key,
                sequence: active.sequence.max(1),
                value: active.value,
            }
        };
        Some(self.handle(session, request, previous, generation))
    }
}

fn preview_target(
    session: &Session,
    target: GestureTarget,
    value: ParameterValue,
    previous: Option<&PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>), SessionError> {
    match target {
        GestureTarget::TrackBalance { track } => {
            session.preview_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.preview_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
        GestureTarget::TrackGain { track } => {
            session.preview_track_gain(track, track_gain_value(value)?, previous)
        }
        GestureTarget::DeviceParameter { device, parameter } => {
            session.preview_device_parameter(device, parameter, value, previous)
        }
        GestureTarget::DeviceParameterPair { .. }
        | GestureTarget::DeviceParameterSnapshot { .. } => Err(SessionError::Invalid(
            "paired device gesture requires two values",
        )),
        GestureTarget::ReturnSend {
            track,
            return_track,
        } => session.preview_return_send(track, return_track, send_gain(value)?, previous),
    }
}

fn commit_target(
    session: &mut Session,
    target: GestureTarget,
    value: ParameterValue,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    match target {
        GestureTarget::TrackBalance { track } => {
            session.commit_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.commit_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
        GestureTarget::TrackGain { track } => {
            session.commit_track_gain(track, track_gain_value(value)?, previous)
        }
        GestureTarget::DeviceParameter { device, parameter } => {
            session.commit_device_parameter(device, parameter, value, previous)
        }
        GestureTarget::DeviceParameterPair { .. }
        | GestureTarget::DeviceParameterSnapshot { .. } => Err(SessionError::Invalid(
            "paired device gesture requires two values",
        )),
        GestureTarget::ReturnSend {
            track,
            return_track,
        } => session.commit_return_send(track, return_track, send_gain(value)?, previous),
    }
}

fn preview_pair_target(
    session: &Session,
    target: GestureTarget,
    value: ParameterPairValue,
    previous: Option<&PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>), SessionError> {
    match target {
        GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => session.preview_device_parameter_pair(
            device,
            (first, value.first),
            (second, value.second),
            previous,
        ),
        _ => Err(SessionError::Invalid(
            "paired values require a paired device gesture",
        )),
    }
}

fn commit_pair_target(
    session: &mut Session,
    target: GestureTarget,
    value: ParameterPairValue,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    match target {
        GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => session.commit_device_parameter_pair(
            device,
            (first, value.first),
            (second, value.second),
            previous,
        ),
        _ => Err(SessionError::Invalid(
            "paired values require a paired device gesture",
        )),
    }
}

fn static_track_balance(session: &Session, track: TrackId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session
        .project()
        .track(track)
        .map(hex_project::Track::balance)
        .ok_or(SessionError::Invalid(
            "track balance target no longer exists",
        ))
}

fn static_return_gain(session: &Session, id: ReturnId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session
        .project()
        .return_tracks()
        .iter()
        .find(|track| track.id() == id)
        .map(hex_project::ReturnTrack::gain)
        .ok_or(SessionError::Invalid("return gain target no longer exists"))
}

fn require_resident_mix(session: &Session) -> Result<(), SessionError> {
    if session.audio_mode() != crate::DocumentAudioMode::Resident {
        return Err(SessionError::Invalid(
            "held mixer controls require resident audio mode",
        ));
    }
    Ok(())
}

fn mix_value(value: ParameterValue, min: f32, max: f32) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(value) if value.is_finite() && (min..=max).contains(&value) => {
            Ok(value)
        }
        _ => Err(SessionError::Invalid(
            "mixer value must be finite and within its range",
        )),
    }
}

fn static_track_gain(session: &Session, track: TrackId) -> Result<f32, SessionError> {
    if session.audio_mode() != crate::DocumentAudioMode::Resident {
        return Err(SessionError::Invalid(
            "held track gain requires resident audio mode",
        ));
    }
    let source = session
        .project()
        .track(track)
        .ok_or(SessionError::Invalid("track gain target no longer exists"))?;
    if !source.gain_envelope().is_empty() || !source.gain_lane().spans().is_empty() {
        return Err(SessionError::Invalid("track gain is owned by automation"));
    }
    let gain = source.gain();
    if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
        return Err(SessionError::Invalid("gain must be finite and within 0..2"));
    }
    Ok(gain)
}

fn track_gain_value(value: ParameterValue) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(gain) => Ok(gain),
        _ => Err(SessionError::Invalid("track gain must be numeric")),
    }
}

fn send_gain(value: ParameterValue) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(gain) if gain.is_finite() && (0.0..=2.0).contains(&gain) => Ok(gain),
        ParameterValue::Float(_) => Err(SessionError::Invalid(
            "return send gain must be finite and between zero and two",
        )),
        _ => Err(SessionError::Invalid("return send gain must be numeric")),
    }
}
