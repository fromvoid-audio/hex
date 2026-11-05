//! Typed node creation on the existing keyed preview lane. Requests carry one
//! node, never an opaque state blob. Existing queue/coalescer capacities and the
//! 832-byte request envelope remain unchanged. Control reconstructs the normal
//! native state and uses the canonical identity-reservation command; callbacks
//! receive only the existing prepared playback/identity metadata.
use super::*;
use hex_engine::eq::{EqState, Node, NodeId};
use hex_lattice_contract::NativeDeviceState;
use hex_project::{Command, DeviceVariant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqNodeCreationSnapshot {
    pub node: Node,
    variant: DeviceVariant,
    next_id: u64,
}
impl EqNodeCreationSnapshot {
    pub fn new(node: Node, variant: DeviceVariant, next_id: u64) -> Result<Self, &'static str> {
        if next_id == 0 || u64::from(node.id.value()) < next_id {
            return Err("invalid eq node identity reservation");
        }
        Ok(Self {
            node,
            variant,
            next_id,
        })
    }
    fn same_binding(&self, other: &Self) -> bool {
        let mut node = self.node;
        node.frequency_hz = other.node.frequency_hz;
        node.gain_db = other.node.gain_db;
        node.q = other.node.q;
        node == other.node && self.variant == other.variant && self.next_id == other.next_id
    }
    pub(super) fn command(
        &self,
        session: &Session,
        device: DeviceId,
        sequence: u64,
    ) -> Result<Command, SessionError> {
        let entry = session
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("eq no longer exists"))?;
        if session.project().next_identity() != self.next_id
            || entry.active_variant() != self.variant
        {
            return Err(SessionError::Invalid("eq node identity or variant changed"));
        }
        if sequence == 0 {
            return Ok(Command::ReplaceDeviceState {
                device,
                state: entry.state().clone(),
            });
        }
        let mut eq = EqState::decode(entry.state())
            .map_err(|_| SessionError::Invalid("eq source is unsupported or malformed"))?;
        let expected = NodeId::new(self.next_id.max(u64::from(eq.next_node_id)))
            .map_err(|_| SessionError::Invalid("eq node identity space exhausted"))?;
        if self.node.id != expected {
            return Err(SessionError::Invalid("eq creation node identity changed"));
        }
        eq.add(self.node)
            .map_err(|_| SessionError::Invalid("invalid eq creation node or node limit reached"))?;
        let reserved_ids = u64::from(eq.next_node_id)
            .checked_sub(self.next_id)
            .ok_or(SessionError::Invalid("eq identity reservation overflow"))?;
        let state = eq
            .encode()
            .and_then(|state| state.with_host_metadata_from(entry.state()))
            .map_err(|_| SessionError::Invalid("eq creation state is unsupported"))?;
        Ok(Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state: entry.state().clone(),
            expected_variant: self.variant,
            state,
            expected_next_id: self.next_id,
            reserved_ids,
        })
    }
}

impl GestureClient {
    pub fn begin_eq_creation(
        &mut self,
        generation: u64,
        device: DeviceId,
        snapshot: EqNodeCreationSnapshot,
    ) -> Result<GestureKey, &'static str> {
        let key = self.begin(
            generation,
            GestureTarget::EqNodeCreation { device },
            ParameterValue::Float(0.0),
        )?;
        let active = self
            .active
            .as_mut()
            .ok_or("eq creation gesture did not begin")?;
        active.creation = Some(snapshot);
        self.pending_begin = Some(GestureRequest::BeginEqCreation {
            key,
            device,
            snapshot,
        });
        Ok(key)
    }
    pub fn change_eq_creation(
        &mut self,
        snapshot: EqNodeCreationSnapshot,
    ) -> Result<u64, &'static str> {
        self.update_eq_creation(snapshot, false)
    }
    pub fn finish_eq_creation(
        &mut self,
        snapshot: EqNodeCreationSnapshot,
    ) -> Result<(), &'static str> {
        self.update_eq_creation(snapshot, true).map(|_| ())
    }
    fn update_eq_creation(
        &mut self,
        snapshot: EqNodeCreationSnapshot,
        terminal: bool,
    ) -> Result<u64, &'static str> {
        if self.has_pending_terminal() {
            return Err("eq creation gesture is finishing");
        }
        let active = self
            .active
            .as_mut()
            .ok_or("no eq creation gesture is active")?;
        if !matches!(active.target, GestureTarget::EqNodeCreation { .. })
            || !active
                .creation
                .is_some_and(|initial| initial.same_binding(&snapshot))
        {
            return Err("eq creation identity, kind or bank changed");
        }
        active.sequence = active.sequence.saturating_add(1).max(1);
        active.creation = Some(snapshot);
        if terminal {
            self.latest_change = None;
            self.terminal = Some(GestureRequest::CommitEqCreation {
                key: active.key,
                sequence: active.sequence,
                snapshot,
            });
        } else {
            self.latest_change = Some(GestureRequest::ChangeEqCreation {
                key: active.key,
                sequence: active.sequence,
                snapshot,
            });
        }
        Ok(active.sequence)
    }
}

impl GestureMachine {
    pub(super) fn apply_eq_creation(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let key = request.key();
        match request {
            GestureRequest::BeginEqCreation {
                device, snapshot, ..
            } => {
                if self.active.is_some()
                    || self.last_key.is_some_and(|last| {
                        last.generation == key.generation && key.token <= last.token
                    })
                {
                    return Err(SessionError::Invalid(
                        "eq creation gesture is active or stale",
                    ));
                }
                // Use the real canonical command's validation before accepting
                // Begin. This private model consumes neither canonical IDs nor history.
                let mut candidate = hex_project::Editor::new(session.project().clone(), 1)?;
                candidate.execute(snapshot.command(session, device, 1)?)?;
                let class = session.native_device_class(device)?;
                let entry = session
                    .project()
                    .device(device)
                    .ok_or(SessionError::Invalid("eq no longer exists"))?;
                self.active = Some(Active {
                    snapshot: GestureSnapshot {
                        track_gains: None,
                        touched_parameters: crate::device_gesture::TouchedParameters::default(),
                        manual_edit: false,
                        key,
                        target: GestureTarget::EqNodeCreation { device },
                        sequence: 0,
                        value: ParameterValue::Float(0.0),
                        secondary_value: None,
                        parameters: None,
                        creation: Some(snapshot),
                    },
                    kind: ActiveKind::Device {
                        class,
                        type_id: entry.state().type_id(),
                    },
                    creation_origin: Some(entry.state().clone()),
                    origin_parameters: None,
                });
                self.last_key = Some(key);
            }
            GestureRequest::ChangeEqCreation {
                sequence, snapshot, ..
            }
            | GestureRequest::CommitEqCreation {
                sequence, snapshot, ..
            } => {
                let active = self.checked_active(session, key)?;
                let terminal = request.phase() == GesturePhase::Commit;
                if sequence == 0
                    || sequence < active.sequence
                    || (!terminal && sequence == active.sequence)
                {
                    return Err(SessionError::Invalid("eq creation update is stale"));
                }
                if !active
                    .creation
                    .is_some_and(|initial| initial.same_binding(&snapshot))
                {
                    return Err(SessionError::Invalid(
                        "eq creation identity, kind or bank changed",
                    ));
                }
                let draft = GestureSnapshot {
                    sequence,
                    creation: Some(snapshot),
                    ..active
                };
                let before = session.project().clone();
                let (project, prepared) = prepare(
                    session,
                    if terminal {
                        GesturePreparation::Commit(draft)
                    } else {
                        GesturePreparation::Preview(draft)
                    },
                )?
                .ok_or(SessionError::Invalid("eq creation produced no revision"))?;
                outcome.playback = Some(prepared);
                if terminal {
                    outcome.canonical_changed = session.project() != &before;
                    self.active = None;
                } else {
                    let current = self
                        .active
                        .as_mut()
                        .ok_or(SessionError::Invalid("eq creation gesture retired"))?;
                    current.snapshot = draft;
                    outcome.preview_project = Some(project);
                }
            }
            _ => return Err(SessionError::Invalid("invalid eq creation request")),
        }
        Ok(())
    }
    pub(super) fn validate_creation_origin(
        &self,
        session: &Session,
        snapshot: GestureSnapshot,
        device: DeviceId,
    ) -> Result<(), SessionError> {
        let origin: &NativeDeviceState = self
            .active
            .as_ref()
            .and_then(|active| active.creation_origin.as_ref())
            .ok_or(SessionError::Invalid("eq creation origin is missing"))?;
        let entry = session
            .project()
            .device(device)
            .filter(|entry| entry.state() == origin)
            .ok_or(SessionError::Invalid("eq creation source changed"))?;
        let creation = snapshot
            .creation
            .ok_or(SessionError::Invalid("eq creation node is missing"))?;
        if entry.active_variant() != creation.variant
            || session.project().next_identity() != creation.next_id
        {
            return Err(SessionError::Invalid("eq node identity or variant changed"));
        }
        Ok(())
    }
}
