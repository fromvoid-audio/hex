//! Worker-side device gesture state machine (`GestureMachine`).
use super::*;

pub(crate) struct GestureOutcome {
    pub ack: GestureAck,
    pub playback: Option<Arc<PreparedRevision>>,
    pub preview_project: Option<Arc<Project>>,
    pub canonical_changed: bool,
}

/// The existing machine validates ordering/targets; the worker prepares and publishes before
/// this operation may advance its active snapshot or canonical history.
#[derive(Clone, Copy)]
pub(crate) enum GesturePreparation {
    Plugin(PluginGesturePreparation),
    Preview(GestureSnapshot),
    /// Repeated paired preview with unchanged values against an already accepted
    /// preview. Owners skip rebuilding/republishing only when the canonical
    /// revision and audio owner still hold that exact preview.
    UnchangedPairPreview(GestureSnapshot),
    Commit(GestureSnapshot),
    Restore,
}
pub(crate) type PreparedGesture =
    Result<Option<(Arc<Project>, Arc<PreparedRevision>)>, SessionError>;

pub(super) struct AcceptedPairPreview {
    previous: Option<std::sync::Weak<PreparedRevision>>,
    audio_mode: crate::DocumentAudioMode,
    semantic_source_epoch: u64,
}

impl AcceptedPairPreview {
    fn capture(session: &Session, previous: Option<&Arc<PreparedRevision>>) -> Self {
        Self {
            previous: previous.map(Arc::downgrade),
            audio_mode: session.audio_mode(),
            semantic_source_epoch: session.semantic_source_epoch,
        }
    }

    fn matches(&self, session: &Session, previous: Option<&Arc<PreparedRevision>>) -> bool {
        if self.audio_mode != session.audio_mode()
            || self.semantic_source_epoch != session.semantic_source_epoch
        {
            return false;
        }
        matches!(
            (&self.previous, previous),
            (Some(saved), Some(current))
                if saved.upgrade().is_some_and(|prev| Arc::ptr_eq(&prev, current))
        )
    }
}

pub(super) struct Active {
    pub(super) snapshot: GestureSnapshot,
    pub(super) kind: ActiveKind,
    pub(super) creation_origin: Option<hex_lattice_contract::NativeDeviceState>,
    pub(super) origin_parameters: Option<DeviceParameterSnapshot>,
    pub(super) state_origin: Option<hex_lattice_contract::NativeDeviceState>,
    pub(super) plugin_initial: Option<PluginParameterValue>,
    pub(super) accepted_pair_preview: Option<AcceptedPairPreview>,
}

#[derive(Clone, Copy)]
pub(super) enum ActiveKind {
    Plugin {
        recording: bool,
    },
    TrackGain,
    TrackGains,
    TrackBalance,
    ReturnGain,
    MainGain,
    Device {
        class: NativeDeviceClass,
        type_id: DeviceTypeId,
    },
    ReturnSend,
}

#[derive(Default)]
pub(crate) struct GestureMachine {
    pub(super) active: Option<Active>,
    pub(super) last_key: Option<GestureKey>,
}

impl GestureRequest {
    fn manual_edit(self) -> bool {
        match self {
            Self::Change { .. } | Self::ChangePair { .. } | Self::ChangeSnapshot { .. } => true,
            Self::Commit { manual_edit, .. } | Self::CommitPair { manual_edit, .. } => manual_edit,
            Self::CommitSnapshot {
                touched_parameters, ..
            } => !touched_parameters.is_empty(),
            _ => false,
        }
    }
    pub fn key(self) -> GestureKey {
        match self {
            Self::Begin { key, .. }
            | Self::BeginTrackGains { key, .. }
            | Self::BeginPlugin { key, .. }
            | Self::ChangePlugin { key, .. }
            | Self::CommitPlugin { key, .. }
            | Self::Change { key, .. }
            | Self::ChangePair { key, .. }
            | Self::Commit { key, .. }
            | Self::CommitPair { key, .. }
            | Self::BeginSnapshot { key, .. }
            | Self::ChangeSnapshot { key, .. }
            | Self::CommitSnapshot { key, .. }
            | Self::BeginEqCreation { key, .. }
            | Self::ChangeEqCreation { key, .. }
            | Self::CommitEqCreation { key, .. }
            | Self::Cancel { key } => key,
        }
    }
    pub fn phase(self) -> GesturePhase {
        match self {
            Self::Begin { .. }
            | Self::BeginTrackGains { .. }
            | Self::BeginSnapshot { .. }
            | Self::BeginEqCreation { .. }
            | Self::BeginPlugin { .. } => GesturePhase::Begin,
            Self::Change { .. } => GesturePhase::Change,
            Self::ChangePlugin { .. } => GesturePhase::Change,
            Self::CommitPlugin { .. } => GesturePhase::Commit,
            Self::ChangePair { .. }
            | Self::ChangeSnapshot { .. }
            | Self::ChangeEqCreation { .. } => GesturePhase::Change,
            Self::Commit { .. } => GesturePhase::Commit,
            Self::CommitPair { .. }
            | Self::CommitSnapshot { .. }
            | Self::CommitEqCreation { .. } => GesturePhase::Commit,
            Self::Cancel { .. } => GesturePhase::Cancel,
        }
    }
    pub fn sequence(self) -> u64 {
        match self {
            Self::Change { sequence, .. }
            | Self::ChangePlugin { sequence, .. }
            | Self::CommitPlugin { sequence, .. }
            | Self::ChangePair { sequence, .. }
            | Self::Commit { sequence, .. }
            | Self::CommitPair { sequence, .. }
            | Self::ChangeSnapshot { sequence, .. }
            | Self::CommitSnapshot { sequence, .. }
            | Self::ChangeEqCreation { sequence, .. }
            | Self::CommitEqCreation { sequence, .. } => sequence,
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

    /// Retire a refused native terminal when the resident worker has no
    /// matching published overlay. Plugin previews use their own custody.
    pub(crate) fn retire_unpublished_native_terminal(&mut self, ack: &mut GestureAck) {
        if ack.preview_retained
            && self.active.as_ref().is_some_and(|active| {
                active.snapshot.key == ack.key && !matches!(active.kind, ActiveKind::Plugin { .. })
            })
        {
            self.active = None;
            ack.preview_retained = false;
        }
    }

    /// An accepted restart relinquishes output-bound previews. An unrecorded
    /// plugin preview has lost its original source, so report that its edit was
    /// not saved. Recorded gestures retain their separate recovery custody.
    pub(crate) fn retire_native_on_restart(&mut self) -> Option<GestureAck> {
        let active = self.active.as_ref()?;
        if matches!(active.kind, ActiveKind::Plugin { recording: true }) {
            return None;
        }
        let plugin = matches!(active.kind, ActiveKind::Plugin { .. });
        let snapshot = active.snapshot;
        self.active = None;
        Some(GestureAck {
            key: snapshot.key,
            phase: GesturePhase::Cancel,
            sequence: 0,
            accepted: false,
            preview_retained: false,
            source_lost: plugin,
            reason: Some(if plugin {
                "vst3 parameter edit was not saved · audio restarted".into()
            } else {
                "device edit canceled · audio is restarting".into()
            }),
        })
    }

    pub fn handle(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> GestureOutcome {
        let same_non_resident_owner = self
            .active
            .as_ref()
            .and_then(|active| active.accepted_pair_preview.as_ref())
            .is_some_and(|witness| witness.matches(session, previous));
        self.handle_using(
            session,
            request,
            previous,
            generation,
            false,
            |session, operation| {
                default_preparation(session, operation, previous, same_non_resident_owner)
            },
        )
    }
    pub(crate) fn handle_prepared(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        prepare: impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> GestureOutcome {
        self.handle_using(session, request, previous, generation, true, prepare)
    }
    pub(crate) fn handle_verified(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        prepare: impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> GestureOutcome {
        self.handle_using(session, request, previous, generation, false, prepare)
    }
    fn handle_using(
        &mut self,
        session: &mut Session,
        request: GestureRequest,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        retain_terminal: bool,
        mut prepare: impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> GestureOutcome {
        let mut outcome = GestureOutcome {
            ack: GestureAck {
                preview_retained: false,
                source_lost: false,
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
        let result = self.apply(
            session,
            request,
            previous,
            generation,
            &mut outcome,
            &mut prepare,
        );
        match result {
            Ok(()) => outcome.ack.accepted = true,
            Err(error) => {
                let source_lost = matches!(error, SessionError::PluginSourceLost(_));
                // A recorded terminal may already have restored authored input
                // before durable take publication refuses. Its completed SDK
                // preview cannot be retained, and the source is still alive.
                let recording_released = matches!(error, SessionError::PluginRecordingReleased(_))
                    && matches!(request.phase(), GesturePhase::Commit | GesturePhase::Cancel)
                    && self.active.as_ref().is_some_and(|active| {
                        matches!(active.kind, ActiveKind::Plugin { recording: true })
                    });
                if (source_lost || recording_released)
                    && self.snapshot().is_some_and(|active| {
                        active.key == request.key()
                            && matches!(active.target, GestureTarget::PluginParameter { .. })
                    })
                {
                    self.active = None;
                    outcome.playback = None;
                    outcome.preview_project = None;
                    outcome.ack.source_lost = source_lost;
                }
                outcome.ack.reason = Some(hex_storage::text::lowercase_outside_verbatim(
                    &error.to_string(),
                ));
                // Native terminal refusal keeps the actual accepted overlay. The legacy
                // adapter retains its existing retirement behavior; stale keys own neither.
                if retain_terminal
                    && matches!(request.phase(), GesturePhase::Commit | GesturePhase::Cancel)
                    && self
                        .snapshot()
                        .is_some_and(|active| active.key == request.key())
                {
                    outcome.ack.preview_retained = true;
                } else if request.phase() == GesturePhase::Commit
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
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let key = request.key();
        if key.generation != generation || key.token == 0 {
            return Err(SessionError::Invalid(
                "device gesture belongs to an old project",
            ));
        }
        match request {
            GestureRequest::BeginPlugin { .. }
            | GestureRequest::ChangePlugin { .. }
            | GestureRequest::CommitPlugin { .. } => {
                self.apply_plugin(session, request, generation, outcome, prepare)
            }
            GestureRequest::BeginEqCreation { .. }
            | GestureRequest::ChangeEqCreation { .. }
            | GestureRequest::CommitEqCreation { .. } => {
                self.apply_eq_creation(session, request, outcome, prepare)
            }
            GestureRequest::BeginTrackGains { group, .. } => {
                self.apply_begin_track_gains(session, key, generation, group)
            }
            GestureRequest::BeginSnapshot {
                device, snapshot, ..
            } => self.apply_begin_snapshot(session, key, generation, device, snapshot),
            GestureRequest::ChangeSnapshot {
                sequence,
                snapshot,
                touched_parameters,
                ..
            } => self.apply_snapshot_update(
                session,
                key,
                sequence,
                snapshot,
                touched_parameters,
                false,
                outcome,
                prepare,
            ),
            GestureRequest::CommitSnapshot {
                sequence,
                snapshot,
                touched_parameters,
                ..
            } => self.apply_snapshot_update(
                session,
                key,
                sequence,
                snapshot,
                touched_parameters,
                true,
                outcome,
                prepare,
            ),
            GestureRequest::Begin { target, .. } => {
                self.apply_begin_target(session, key, generation, target)
            }
            GestureRequest::Change {
                sequence, value, ..
            } => self.apply_scalar_change(session, key, sequence, value, outcome, prepare),
            GestureRequest::ChangePair {
                sequence, value, ..
            } => self.apply_pair_change(session, key, sequence, value, previous, outcome, prepare),
            GestureRequest::Commit {
                sequence, value, ..
            } => self.apply_scalar_commit(
                session,
                key,
                sequence,
                value,
                request.manual_edit(),
                outcome,
                prepare,
            ),
            GestureRequest::CommitPair {
                sequence, value, ..
            } => self.apply_pair_commit(
                session,
                key,
                sequence,
                value,
                request.manual_edit(),
                outcome,
                prepare,
            ),
            GestureRequest::Cancel { .. } => self.apply_cancel(session, key, outcome, prepare),
        }
    }

    fn apply_begin_track_gains(
        &mut self,
        session: &Session,
        key: GestureKey,
        generation: u64,
        group: hex_project::RelativeTrackGains,
    ) -> Result<(), SessionError> {
        if self.active.is_some()
            || self
                .last_key
                .is_some_and(|last| last.generation == generation && key.token <= last.token)
        {
            return Err(SessionError::Invalid(
                "selected gain gesture is active or stale",
            ));
        }
        group.validate_current(session.project())?;
        self.active = Some(Active {
            snapshot: GestureSnapshot {
                touched_parameters: TouchedParameters::default(),
                manual_edit: false,
                key,
                target: GestureTarget::TrackGains {
                    track: group.anchor(),
                },
                sequence: 0,
                value: ParameterValue::Float(group.anchor_gain()),
                secondary_value: None,
                parameters: None,
                creation: None,
                track_gains: Some(group),
                plugin_value: None,
            },
            kind: ActiveKind::TrackGains,
            creation_origin: None,
            origin_parameters: None,
            state_origin: None,
            plugin_initial: None,
            accepted_pair_preview: None,
        });
        self.last_key = Some(key);
        Ok(())
    }

    fn apply_begin_snapshot(
        &mut self,
        session: &Session,
        key: GestureKey,
        generation: u64,
        device: DeviceId,
        snapshot: DeviceParameterSnapshot,
    ) -> Result<(), SessionError> {
        if self.active.is_some()
            || self
                .last_key
                .is_some_and(|last| last.generation == generation && key.token <= last.token)
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
                track_gains: None,
                touched_parameters: crate::device_gesture::TouchedParameters::default(),
                manual_edit: false,
                key,
                target: GestureTarget::DeviceParameterSnapshot { device },
                sequence: 0,
                value: ParameterValue::Float(0.0),
                secondary_value: None,
                parameters: Some(initial),
                creation: None,
                plugin_value: None,
            },
            kind: ActiveKind::Device {
                class,
                type_id: state.type_id(),
            },
            creation_origin: None,
            origin_parameters: Some(initial),
            state_origin: None,
            plugin_initial: None,
            accepted_pair_preview: None,
        });
        self.last_key = Some(key);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_snapshot_update(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        sequence: u64,
        snapshot: DeviceParameterSnapshot,
        requested_touched: TouchedParameters,
        terminal: bool,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let active = self.checked_active(session, key)?;
        if sequence == 0 || sequence < active.sequence || (!terminal && sequence == active.sequence)
        {
            return Err(SessionError::Invalid("snapshot gesture update is stale"));
        }
        if !active
            .parameters
            .is_some_and(|value| value.same_parameters(&snapshot))
        {
            return Err(SessionError::Invalid("snapshot gesture parameters changed"));
        }
        let GestureTarget::DeviceParameterSnapshot { .. } = active.target else {
            return Err(SessionError::Invalid("snapshot gesture target changed"));
        };
        let origin = self
            .active
            .as_ref()
            .and_then(|active| active.origin_parameters)
            .ok_or(SessionError::Invalid("snapshot origin is missing"))?;
        if !origin.same_parameters(&snapshot) {
            return Err(SessionError::Invalid("snapshot gesture parameters changed"));
        }
        let mut touched = active.touched_parameters;
        touched
            .merge(requested_touched, origin)
            .map_err(SessionError::Invalid)?;
        touched
            .include_changed(origin, snapshot)
            .map_err(SessionError::Invalid)?;
        if terminal {
            let before = session.project().clone();
            let (_, prepared) = prepare(
                session,
                GesturePreparation::Commit(GestureSnapshot {
                    manual_edit: !touched.is_empty(),
                    touched_parameters: touched,
                    sequence,
                    parameters: Some(snapshot),
                    ..active
                }),
            )?
            .ok_or(SessionError::Invalid("gesture commit produced no revision"))?;
            outcome.playback = Some(prepared);
            outcome.canonical_changed = session.project() != &before;
            self.active = None;
        } else {
            let (project, prepared) = prepare(
                session,
                GesturePreparation::Preview(GestureSnapshot {
                    manual_edit: !touched.is_empty(),
                    touched_parameters: touched,
                    sequence,
                    parameters: Some(snapshot),
                    ..active
                }),
            )?
            .ok_or(SessionError::Invalid(
                "gesture preview produced no revision",
            ))?;
            let current = self
                .active
                .as_mut()
                .ok_or(SessionError::Invalid("gesture ended during preview"))?;
            current.snapshot.manual_edit = !touched.is_empty();
            current.snapshot.touched_parameters = touched;
            current.snapshot.sequence = sequence;
            current.snapshot.parameters = Some(snapshot);
            outcome.preview_project = Some(project);
            outcome.playback = Some(prepared);
        }
        Ok(())
    }

    fn apply_begin_target(
        &mut self,
        session: &Session,
        key: GestureKey,
        generation: u64,
        target: GestureTarget,
    ) -> Result<(), SessionError> {
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
            GestureTarget::TrackGains { .. }
            | GestureTarget::DeviceParameterSnapshot { .. }
            | GestureTarget::EqNodeCreation { .. } => {
                return Err(SessionError::Invalid(
                    "snapshot gesture requires initial parameters",
                ));
            }
            GestureTarget::PluginParameter { .. } => {
                return Err(SessionError::Invalid(
                    "plugin gesture requires source context",
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
            GestureTarget::MainGain => {
                require_resident_mix(session)?;
                (
                    ActiveKind::MainGain,
                    ParameterValue::Float(session.project().master_gain()),
                    None,
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
                    return Err(SessionError::Invalid("return send target no longer exists"));
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
                track_gains: None,
                touched_parameters: crate::device_gesture::TouchedParameters::default(),
                manual_edit: false,
                key,
                target,
                sequence: 0,
                value,
                secondary_value,
                parameters: None,
                creation: None,
                plugin_value: None,
            },
            kind,
            creation_origin: None,
            origin_parameters: None,
            state_origin: None,
            plugin_initial: None,
            accepted_pair_preview: None,
        });
        self.last_key = Some(key);
        Ok(())
    }

    fn apply_scalar_change(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let active = self.checked_active(session, key)?;
        if active.plugin_value.is_some() {
            return Err(SessionError::Invalid(
                "plugin gesture requires lossless values",
            ));
        }
        if sequence == 0 || sequence <= active.sequence {
            return Err(SessionError::Invalid("device gesture update is stale"));
        }
        let (project, prepared) = prepare(
            session,
            GesturePreparation::Preview(GestureSnapshot {
                manual_edit: true,
                sequence,
                value,
                ..active
            }),
        )?
        .ok_or(SessionError::Invalid(
            "gesture preview produced no revision",
        ))?;
        if let Some(current) = self.active.as_mut() {
            current.snapshot.manual_edit = true;
            current.snapshot.sequence = sequence;
            current.snapshot.value = value;
        }
        outcome.preview_project = Some(project);
        outcome.playback = Some(prepared);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_pair_change(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        sequence: u64,
        value: ParameterPairValue,
        previous: Option<&Arc<PreparedRevision>>,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let active = self.checked_active(session, key)?;
        if active.plugin_value.is_some() {
            return Err(SessionError::Invalid(
                "plugin gesture requires lossless values",
            ));
        }
        if sequence == 0 || sequence <= active.sequence {
            return Err(SessionError::Invalid("device gesture update is stale"));
        }
        let draft = GestureSnapshot {
            manual_edit: true,
            sequence,
            value: value.first,
            secondary_value: Some(value.second),
            ..active
        };
        let unchanged = self.active.as_ref().is_some_and(|current| {
            current.accepted_pair_preview.is_some()
                && current.snapshot.manual_edit
                && current.snapshot.sequence > 0
                && current.snapshot.value == value.first
                && current.snapshot.secondary_value == Some(value.second)
        });
        let operation = if unchanged {
            GesturePreparation::UnchangedPairPreview(draft)
        } else {
            GesturePreparation::Preview(draft)
        };
        match prepare(session, operation)? {
            Some((project, prepared)) => {
                let witness = AcceptedPairPreview::capture(session, previous);
                if let Some(current) = self.active.as_mut() {
                    current.snapshot.manual_edit = true;
                    current.snapshot.sequence = sequence;
                    current.snapshot.value = value.first;
                    current.snapshot.secondary_value = Some(value.second);
                    current.accepted_pair_preview = Some(witness);
                }
                outcome.preview_project = Some(project);
                outcome.playback = Some(prepared);
            }
            None if unchanged => {
                if let Some(current) = self.active.as_mut() {
                    current.snapshot.sequence = sequence;
                }
            }
            None => {
                return Err(SessionError::Invalid(
                    "gesture preview produced no revision",
                ));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_scalar_commit(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        sequence: u64,
        value: ParameterValue,
        manual_edit: bool,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let active = self.checked_active(session, key)?;
        if active.plugin_value.is_some() {
            return Err(SessionError::Invalid(
                "plugin gesture requires lossless values",
            ));
        }
        if sequence == 0 || sequence < active.sequence {
            return Err(SessionError::Invalid("device gesture commit is stale"));
        }
        let before = session.project().clone();
        let (_, prepared) = prepare(
            session,
            GesturePreparation::Commit(GestureSnapshot {
                manual_edit: active.manual_edit || manual_edit || active.value != value,
                sequence,
                value,
                ..active
            }),
        )?
        .ok_or(SessionError::Invalid("gesture commit produced no revision"))?;
        outcome.canonical_changed = session.project() != &before;
        outcome.playback = Some(prepared);
        self.active = None;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_pair_commit(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        sequence: u64,
        value: ParameterPairValue,
        manual_edit: bool,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let active = self.checked_active(session, key)?;
        if active.plugin_value.is_some() {
            return Err(SessionError::Invalid(
                "plugin gesture requires lossless values",
            ));
        }
        if sequence == 0 || sequence < active.sequence {
            return Err(SessionError::Invalid("device gesture commit is stale"));
        }
        let before = session.project().clone();
        let (_, prepared) = prepare(
            session,
            GesturePreparation::Commit(GestureSnapshot {
                manual_edit: active.manual_edit
                    || manual_edit
                    || active.value != value.first
                    || active.secondary_value != Some(value.second),
                sequence,
                value: value.first,
                secondary_value: Some(value.second),
                ..active
            }),
        )?
        .ok_or(SessionError::Invalid("gesture commit produced no revision"))?;
        outcome.canonical_changed = session.project() != &before;
        outcome.playback = Some(prepared);
        self.active = None;
        Ok(())
    }

    fn apply_cancel(
        &mut self,
        session: &mut Session,
        key: GestureKey,
        outcome: &mut GestureOutcome,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Result<(), SessionError> {
        let snapshot = self.checked_key(key)?;
        let operation = if let Some(initial) = self
            .active
            .as_ref()
            .and_then(|active| active.plugin_initial)
        {
            GesturePreparation::Plugin(PluginGesturePreparation::Restore { snapshot, initial })
        } else {
            GesturePreparation::Restore
        };
        let prepared = prepare(session, operation)?;
        self.active = None;
        outcome.playback = prepared.map(|(_, prepared)| prepared);
        Ok(())
    }

    pub(super) fn checked_key(&self, key: GestureKey) -> Result<GestureSnapshot, SessionError> {
        self.snapshot()
            .filter(|active| active.key == key)
            .ok_or(SessionError::Invalid("device gesture is no longer active"))
    }
    pub(super) fn checked_active(
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
            (GestureTarget::EqNodeCreation { device }, ActiveKind::Device { class, .. }) => {
                self.validate_creation_origin(session, snapshot, device)?;
                if session.native_device_class(device)? != class {
                    return Err(SessionError::Invalid(
                        "eq creation target changed native class",
                    ));
                }
            }
            (GestureTarget::MainGain, ActiveKind::MainGain) => {
                require_resident_mix(session)?;
            }
            (GestureTarget::TrackBalance { track }, ActiveKind::TrackBalance) => {
                static_track_balance(session, track)?;
            }
            (GestureTarget::ReturnGain { return_track }, ActiveKind::ReturnGain) => {
                static_return_gain(session, return_track)?;
            }
            (GestureTarget::TrackGains { track }, ActiveKind::TrackGains) => {
                let group = snapshot
                    .track_gains
                    .filter(|group| group.anchor() == track)
                    .ok_or(SessionError::Invalid("selected gain cohort changed"))?;
                group.validate_current(session.project())?;
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
            (GestureTarget::PluginParameter { device, .. }, ActiveKind::Plugin { .. }) => {
                let origin = active
                    .state_origin
                    .as_ref()
                    .ok_or(SessionError::Invalid("plugin gesture origin is missing"))?;
                if snapshot.plugin_value.is_none()
                    || session
                        .project()
                        .device(device)
                        .is_none_or(|device| !device.state().processing_eq(origin))
                {
                    return Err(SessionError::PluginSourceLost(
                        "plugin gesture target was removed or replaced".into(),
                    ));
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
        if active.plugin_value.is_some() {
            return self.rebase_prepared(session, previous, &mut |_, _| {
                Err(SessionError::PluginSourceLost(
                    "plugin gesture requires its original native host".into(),
                ))
            });
        }
        let result = self.checked_active(session, active.key).and_then(|_| {
            if active.track_gains.is_some()
                || active.creation.is_some()
                || active.manual_edit
                    && matches!(
                        active.target,
                        GestureTarget::DeviceParameter { .. }
                            | GestureTarget::DeviceParameterPair { .. }
                            | GestureTarget::DeviceParameterSnapshot { .. }
                    )
            {
                session.preview_mix_command(
                    command_for_snapshot(session, active)?,
                    previous.map(Arc::as_ref),
                )
            } else if let Some(parameters) = active.parameters {
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
            Ok((project, prepared)) => {
                if let Some(current) = self.active.as_mut()
                    && matches!(
                        current.snapshot.target,
                        GestureTarget::DeviceParameterPair { .. }
                    )
                    && current.snapshot.manual_edit
                    && current.snapshot.sequence > 0
                {
                    current.accepted_pair_preview =
                        Some(AcceptedPairPreview::capture(session, previous));
                }
                Some(GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: false,
                        key: active.key,
                        phase: GesturePhase::Change,
                        sequence: active.sequence,
                        accepted: true,
                        reason: None,
                    },
                    playback: Some(prepared),
                    preview_project: Some(project),
                    canonical_changed: false,
                })
            }
            Err(error) => {
                let mut result = self.handle(
                    session,
                    GestureRequest::Cancel { key: active.key },
                    previous,
                    generation,
                );
                result.ack.reason = Some(hex_storage::text::lowercase_outside_verbatim(
                    &error.to_string(),
                ));
                Some(result)
            }
        }
    }

    pub(crate) fn rebase_prepared(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        prepare: &mut impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        let plugin = active.plugin_value.is_some();
        let result = self
            .checked_active(session, active.key)
            .and_then(|_| {
                prepare(
                    session,
                    if plugin {
                        GesturePreparation::Plugin(PluginGesturePreparation::Preview(active))
                    } else {
                        GesturePreparation::Preview(active)
                    },
                )
            })
            .and_then(|prepared| {
                if prepared.is_none() && !plugin {
                    Err(SessionError::Invalid(
                        "gesture preview produced no revision",
                    ))
                } else {
                    Ok(prepared)
                }
            });
        match result {
            Ok(prepared) => {
                if let Some(current) = self.active.as_mut()
                    && matches!(
                        current.snapshot.target,
                        GestureTarget::DeviceParameterPair { .. }
                    )
                    && current.snapshot.manual_edit
                    && current.snapshot.sequence > 0
                {
                    current.accepted_pair_preview =
                        Some(AcceptedPairPreview::capture(session, previous));
                }
                let (project, playback) = match prepared {
                    Some((project, playback)) => (Some(project), Some(playback)),
                    None => (None, None),
                };
                Some(GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: false,
                        key: active.key,
                        phase: GesturePhase::Change,
                        sequence: active.sequence,
                        accepted: true,
                        reason: None,
                    },
                    playback,
                    preview_project: (!plugin).then_some(project).flatten(),
                    canonical_changed: false,
                })
            }
            Err(error) if plugin && !matches!(error, SessionError::PluginSourceLost(_)) => {
                // The independent canonical publication can retain this same
                // native instance. A temporary validation refusal cannot erase
                // its admitted SDK preview or pretend the source disappeared.
                Some(GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: false,
                        key: active.key,
                        phase: GesturePhase::Change,
                        sequence: active.sequence,
                        accepted: false,
                        reason: Some(error.to_string()),
                    },
                    playback: None,
                    preview_project: None,
                    canonical_changed: false,
                })
            }
            Err(error) => {
                // The caller has already accepted an independent canonical graph, so its normal
                // publication retires this invalid overlay. Never claim that edit rolled back.
                self.active = None;
                Some(GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: plugin,
                        key: active.key,
                        phase: GesturePhase::Cancel,
                        sequence: 0,
                        accepted: false,
                        reason: Some(error.to_string()),
                    },
                    playback: previous.cloned(),
                    preview_project: None,
                    canonical_changed: false,
                })
            }
        }
    }
    pub(crate) fn finish_prepared(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        prepare: impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        let request = terminal_request(active);
        Some(self.handle_prepared(session, request, previous, generation, prepare))
    }

    pub(crate) fn finish_verified(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
        prepare: impl FnMut(&mut Session, GesturePreparation) -> PreparedGesture,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        let request = terminal_request(active);
        Some(self.handle_verified(session, request, previous, generation, prepare))
    }

    pub fn finish(
        &mut self,
        session: &mut Session,
        previous: Option<&Arc<PreparedRevision>>,
        generation: u64,
    ) -> Option<GestureOutcome> {
        let active = self.snapshot()?;
        let request = terminal_request(active);
        Some(self.handle(session, request, previous, generation))
    }
}
