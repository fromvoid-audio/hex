//! Control-side connection from one keyed SDK gesture to the existing device
//! take Lane. The Lane retains disk custody; this owner retains at most one
//! original observation under backpressure, at most two later SDK observations
//! during failed-input release, and one terminal request while disk
//! completion is pending. No callback, UI clock or second writer lives here.
use super::*;
use crate::device_gesture::{PluginGesturePreparation, PreparedGesture};
use native_automation_recording::{Lane, Step};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    active: Option<Active>,
    terminal: Option<GestureRequest>,
    // A sealed interrupted source may retire before the actor's terminal is
    // dispatched. Retain its resource-free identity until that actor retires.
    retired: Option<crate::device_gesture::GestureKey>,
    // Keep any unadmitted original receipt after failure. New recording must
    // not reuse this custody; the Lane independently retains its writer input.
    failed: Option<Active>,
}
struct Active {
    key: crate::device_gesture::GestureKey,
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    has_points: bool,
    path: PathBuf,
    source_lost: bool,
    pending: Option<DeviceAutomationTakeAction>,
    released: bool,
    ended: bool,
    failed_observations: [Option<PluginParameterObservation>; 2],
    interrupted: bool,
    sealing: bool,
    outcome: Option<Result<Step, String>>,
    deadline: Option<Instant>,
}

impl Active {
    fn terminal_error(&self, error: SessionError) -> SessionError {
        if self.source_lost {
            SessionError::PluginSourceLost(error.to_string())
        } else if self.ended
            && !self.interrupted
            && !matches!(error, SessionError::PluginSourceLost(_))
        {
            SessionError::PluginRecordingReleased(error.to_string())
        } else {
            error
        }
    }
}

pub(super) struct Preparation<'a> {
    pub state: &'a mut State,
    pub lane: &'a mut Lane,
    pub document_base: Option<&'a Path>,
    pub recovery: Option<&'a Path>,
    pub may_defer: bool,
    pub deferred: bool,
}

pub(super) fn start_failed_copy(
    state: &State,
    lane: &mut Lane,
    scope: DeviceAutomationTakeScope,
    destination: &Path,
    max_events: u64,
    gesture_active: bool,
) -> Result<Step, String> {
    if gesture_active {
        return Err("vst3 recovery requires a quiescent gesture".into());
    }
    let source = lane.failed_source(scope)?;
    let observations = state.failed_observations(scope, source)?;
    lane.copy_failed_plugin(scope, destination, max_events, observations)
}

pub(super) fn generic_sealed_recovery_idle(state: &State, lane: &Lane) -> bool {
    !state.pending() && state.failed.is_none() && !lane.busy() && !lane.has_failed_custody()
}

/// Discover original custody without changing the writer, SDK receipts or files.
pub(super) fn failed_custody(
    state: &State,
    lane: &Lane,
    gesture_active: bool,
) -> Result<DeviceAutomationTakeProgress, String> {
    use DeviceAutomationTakeProgress as Progress;
    let Some((scope, path)) = lane.failed_plugin_identity() else {
        return Ok(
            if state.failed.is_some()
                || state
                    .active
                    .as_ref()
                    .is_some_and(|active| active.outcome.as_ref().is_some_and(Result::is_err))
            {
                Progress::FailedPluginPending
            } else {
                Progress::FailedPluginUnavailable
            },
        );
    };
    if gesture_active
        || state.active.is_some()
        || state.terminal.is_some()
        || state.failed.as_ref().is_some_and(|failed| !failed.ended)
    {
        return Ok(Progress::FailedPluginPending);
    }
    let source = lane.failed_source(scope)?;
    if state
        .failed
        .as_ref()
        .is_some_and(|failed| failed.path != path)
    {
        return Err("vst3 recovery draft identities disagree".into());
    }
    state.failed_observations(scope, source)?;
    Ok(Progress::FailedPluginCustody {
        scope,
        path: path.to_owned(),
    })
}

pub(super) fn inspect_failed_copy(
    state: &State,
    lane: &Lane,
    scope: DeviceAutomationTakeScope,
    gesture_active: bool,
) -> Result<Step, String> {
    if gesture_active {
        return Err("vst3 recovery requires a quiescent gesture".into());
    }
    let source = lane.failed_source(scope)?;
    let observations = state.failed_observations(scope, source)?;
    lane.inspect_failed_plugin_copy(scope, &observations)
}

pub(super) struct RecoveryAdoption {
    scope: DeviceAutomationTakeScope,
    pub(super) path: PathBuf,
    receipt: automation_draft::RecoveryCopyReceipt,
    outcome: Arc<automation_draft::DraftWorkerOutcome>,
}

pub(super) fn prepare_failed_adoption(
    state: &State,
    lane: &Lane,
    session: &Session,
    document_base: Option<&Path>,
    scope: DeviceAutomationTakeScope,
    live_source: Option<DeviceAutomationSource>,
    max_events: u64,
) -> Result<(Box<hex_project::Command>, RecoveryAdoption), String> {
    if Some(lane.failed_source(scope)?) != live_source {
        return Err("vst3 recovery live source changed".into());
    }
    let Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy { path, receipt }) =
        inspect_failed_copy(state, lane, scope, false)?
    else {
        return Err("verified VST3 recovery copy is unavailable".into());
    };
    let command = native_automation_recording::prepare_verified_vst3_recovery_on_control(
        session,
        document_base,
        scope,
        &path,
        receipt,
        max_events,
    )?;
    let outcome = Arc::clone(lane.failed_outcome(scope)?);
    Ok((
        Box::new(command),
        RecoveryAdoption {
            scope,
            path,
            receipt,
            outcome,
        },
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryPublication {
    Refused,
    Unchanged,
    Committed,
}

pub(super) fn classify_recovery_publication(
    error: bool,
    previous_revision: u64,
    current_revision: u64,
    callback_confirmed: bool,
) -> RecoveryPublication {
    if error {
        RecoveryPublication::Refused
    } else if previous_revision == current_revision {
        RecoveryPublication::Unchanged
    } else if callback_confirmed && previous_revision.checked_add(1) == Some(current_revision) {
        RecoveryPublication::Committed
    } else {
        RecoveryPublication::Refused
    }
}

/// Retain both owners unless the confirmed resident transaction changed history.
pub(super) fn finish_failed_adoption(
    state: &mut State,
    lane: &mut Lane,
    adoption: &RecoveryAdoption,
    publication: RecoveryPublication,
) -> Result<(), String> {
    match publication {
        RecoveryPublication::Refused | RecoveryPublication::Unchanged => Ok(()),
        RecoveryPublication::Committed => release_failed_adoption(state, lane, adoption),
    }
}

/// Called only after the resident transaction confirmed application and committed.
pub(super) fn release_failed_adoption(
    state: &mut State,
    lane: &mut Lane,
    adoption: &RecoveryAdoption,
) -> Result<(), String> {
    let source = lane.failed_source(adoption.scope)?;
    if !Arc::ptr_eq(lane.failed_outcome(adoption.scope)?, &adoption.outcome) {
        return Err("original VST3 recovery custody changed".into());
    }
    let observations = state.failed_observations(adoption.scope, source)?;
    lane.release_failed_plugin_copy(
        adoption.scope,
        &adoption.path,
        adoption.receipt,
        &observations,
    )?;
    state.failed = None;
    Ok(())
}

fn snapshot(operation: PluginGesturePreparation) -> GestureSnapshot {
    match operation {
        PluginGesturePreparation::Begin { snapshot, .. }
        | PluginGesturePreparation::Preview(snapshot)
        | PluginGesturePreparation::Commit(snapshot)
        | PluginGesturePreparation::Restore { snapshot, .. } => snapshot,
    }
}
impl State {
    /// Copy the bounded receipts only after the original gesture is quiescent.
    /// The owner retains every original; an empty result never clears custody.
    pub(super) fn failed_observations(
        &self,
        scope: DeviceAutomationTakeScope,
        source: DeviceAutomationSource,
    ) -> Result<Vec<PluginParameterObservation>, String> {
        if self.active.is_some() || self.terminal.is_some() {
            return Err("vst3 recovery requires a quiescent gesture".into());
        }
        let Some(failed) = self.failed.as_ref() else {
            return Ok(Vec::new());
        };
        if !failed.ended || failed.scope != scope || failed.source != source {
            return Err("vst3 recovery custody is incomplete or belongs to another source".into());
        }
        let mut observations = Vec::with_capacity(3);
        if let Some(pending) = failed.pending.as_ref() {
            let DeviceAutomationTakeAction::PluginObservation(observation) = pending else {
                return Err("vst3 recovery has an unsupported pending action".into());
            };
            observations.push(**observation);
        }
        let mut gap = false;
        for observation in failed.failed_observations {
            match observation {
                Some(_) if gap => return Err("vst3 recovery observation custody has a gap".into()),
                Some(observation) => observations.push(observation),
                None => gap = true,
            }
        }
        Ok(observations)
    }

    pub(super) fn terminal_for(&self, snapshot: Option<GestureSnapshot>) -> Option<GestureRequest> {
        let snapshot = snapshot
            .filter(|snapshot| self.owns(Some(*snapshot)) || self.retired == Some(snapshot.key))?;
        self.terminal.or_else(|| {
            Some(GestureRequest::CommitPlugin {
                key: snapshot.key,
                sequence: snapshot.sequence.max(1),
                value: snapshot.plugin_value?,
            })
        })
    }
    pub(super) fn owns(&self, snapshot: Option<GestureSnapshot>) -> bool {
        self.active
            .as_ref()
            .zip(snapshot)
            .is_some_and(|(active, snapshot)| active.key == snapshot.key)
    }
    pub(super) fn actor_began(&mut self) {
        self.retired = None;
    }
    pub(super) fn attempt(&mut self, request: GestureRequest) {
        if self.retired == Some(request.key())
            && matches!(
                request.phase(),
                crate::device_gesture::GesturePhase::Commit
                    | crate::device_gesture::GesturePhase::Cancel
            )
        {
            self.retired = None;
        }
        if self.terminal.is_some_and(|pending| {
            pending.key() == request.key()
                && pending.phase() == request.phase()
                && pending.sequence() == request.sequence()
        }) {
            self.terminal = None;
        }
    }
    pub(super) fn defer(&mut self, request: GestureRequest) {
        self.terminal = Some(request);
        if let Some(active) = self.active.as_mut() {
            active
                .deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
        }
    }
    pub(super) fn ready(&self) -> Option<GestureRequest> {
        self.terminal.filter(|request| {
            self.active.as_ref().is_none_or(|active| {
                active.key != request.key()
                    || active.outcome.is_some()
                    || (!active.sealing && active.pending.is_none())
            })
        })
    }
    pub(super) fn pending(&self) -> bool {
        self.active.is_some()
    }
}
impl Preparation<'_> {
    pub(super) fn handles(&self, operation: PluginGesturePreparation) -> bool {
        if let PluginGesturePreparation::Begin { recording, .. } = operation {
            return recording;
        }
        self.state
            .active
            .as_ref()
            .is_some_and(|active| active.key == snapshot(operation).key)
            || self.state.retired == Some(snapshot(operation).key)
    }
    fn pending_result(&mut self) -> PreparedGesture {
        self.deferred = self.may_defer;
        Err(SessionError::Invalid(
            "vst3 recording is waiting for durable take admission",
        ))
    }
    pub(super) fn before(
        &mut self,
        session: &mut Session,
        operation: PluginGesturePreparation,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        current: &mut Option<resident_audio::Snapshot>,
        admission: sampler_export::Admission<'_>,
    ) -> Option<PreparedGesture> {
        if matches!(operation, PluginGesturePreparation::Begin { .. }) {
            return (self.lane.busy()
                || self.state.active.is_some()
                || self.state.failed.is_some()
                || self.state.terminal.is_some())
            .then(|| {
                Err(SessionError::Invalid(
                    "recover or finish the previous automation take first",
                ))
            });
        }
        self.drive(session, context, sink);
        if self.state.retired == Some(snapshot(operation).key) {
            return Some(Err(SessionError::PluginSourceLost(
                "vst3 recording source ended · original take retained for recovery".into(),
            )));
        }
        if self
            .state
            .active
            .as_ref()
            .is_some_and(|active| matches!(active.outcome, Some(Err(_))))
        {
            if !self.may_defer {
                return Some(Err(SessionError::Invalid(
                    "vst3 take writer failed; release the gesture to retain recovery",
                )));
            }
            self.retain_failed_observations(context, sink);
            if !self
                .state
                .active
                .as_ref()
                .is_some_and(|active| active.ended)
            {
                let snapshot = snapshot(operation);
                let Some(initial) = snapshot.plugin_value else {
                    return Some(Err(SessionError::Invalid(
                        "plugin recording value is unavailable",
                    )));
                };
                let device = match snapshot.target {
                    crate::device_gesture::GestureTarget::PluginParameter { device, .. } => device,
                    _ => {
                        return Some(Err(SessionError::Invalid(
                            "plugin recording target changed",
                        )));
                    }
                };
                let Some(device) = session.project().device(device) else {
                    return Some(Err(SessionError::PluginSourceLost(
                        "plugin recording device changed".into(),
                    )));
                };
                let class = observation_class(session, snapshot);
                let result = sink.prepare_plugin_parameter_gesture(PluginParameterGestureInput {
                    context,
                    operation: PluginGesturePreparation::Restore { snapshot, initial },
                    canonical_state: device.state(),
                    reading_class: class,
                    isolated_class: class,
                    confirm_delivery: true,
                    record_touch: true,
                    instrument: session
                        .project()
                        .native_sources()
                        .iter()
                        .any(|source| source.instrument().id() == device.id()),
                });
                self.retain_failed_observations(context, sink);
                if let Err(PluginParameterGestureError::SourceLost(reason)) = result {
                    return Some(Err(SessionError::PluginSourceLost(reason)));
                }
                if !self
                    .state
                    .active
                    .as_ref()
                    .is_some_and(|active| active.ended)
                {
                    if self.state.active.as_ref().is_some_and(|active| {
                        active
                            .deadline
                            .is_some_and(|deadline| Instant::now() >= deadline)
                    }) {
                        return Some(Err(SessionError::Invalid(
                            "vst3 restoration is still pending; retry release to recover the retained take",
                        )));
                    }
                    return Some(self.pending_result());
                }
            }
            return Some(self.finish(session, operation, context, sink, current, admission));
        }
        if self
            .state
            .active
            .as_ref()
            .is_some_and(|active| active.sealing || active.outcome.is_some())
        {
            if matches!(operation, PluginGesturePreparation::Preview(_)) {
                return Some(Err(SessionError::Invalid("vst3 recording is finishing")));
            }
            return Some(self.finish(session, operation, context, sink, current, admission));
        }
        self.state
            .active
            .as_ref()
            .is_some_and(|active| active.pending.is_some())
            .then(|| self.pending_result())
    }
    #[allow(clippy::too_many_arguments)] // Original host result plus the canonical publication owners.
    pub(super) fn after(
        &mut self,
        session: &mut Session,
        operation: PluginGesturePreparation,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        result: Result<PluginParameterGestureResult, PluginParameterGestureError>,
        current: &mut Option<resident_audio::Snapshot>,
        admission: sampler_export::Admission<'_>,
    ) -> PreparedGesture {
        if let PluginGesturePreparation::Begin { snapshot, .. } = operation {
            result.map_err(host_error)?;
            let admitted = (|| {
                let observation = sink
                    .take_plugin_parameter_observation()
                    .ok_or("vst3 recording did not provide its original ready observation")?;
                let scope = DeviceAutomationTakeScope::from_plugin_ready(context, &observation)?;
                let PluginParameterObservation::Ready { key, source, .. } = observation else {
                    return Err("vst3 recording did not begin with ready".into());
                };
                if key != snapshot.key {
                    return Err("vst3 recording ready belongs to another gesture".into());
                }
                let directory = match self.recovery.and_then(Path::parent) {
                    Some(directory) => directory.join("automation-drafts"),
                    None => automation_draft::default_directory()?,
                };
                self.lane
                    .begin(session, self.document_base, scope, source, &directory)?;
                self.state.active = Some(Active {
                    key,
                    scope,
                    source,
                    has_points: false,
                    path: self
                        .lane
                        .path()
                        .ok_or("admitted automation draft has no recovery identity")?
                        .to_owned(),
                    source_lost: false,
                    pending: None,
                    released: false,
                    ended: false,
                    failed_observations: [None; 2],
                    interrupted: false,
                    sealing: false,
                    outcome: None,
                    deadline: None,
                });
                self.state.retired = None;
                Ok::<_, String>(())
            })();
            if let Err(reason) = admitted {
                // No SDK point has been sent. Release the admitted empty hold
                // before refusing Begin; no invented clock or take is created.
                let initial = snapshot
                    .plugin_value
                    .ok_or(SessionError::Invalid("plugin initial value is unavailable"))?;
                let device = session
                    .project()
                    .device(match snapshot.target {
                        crate::device_gesture::GestureTarget::PluginParameter {
                            device, ..
                        } => device,
                        _ => return Err(SessionError::Invalid("plugin recording target changed")),
                    })
                    .ok_or(SessionError::Invalid("plugin recording device changed"))?;
                let _ = sink.prepare_plugin_parameter_gesture(PluginParameterGestureInput {
                    context,
                    operation: PluginGesturePreparation::Restore { snapshot, initial },
                    canonical_state: device.state(),
                    reading_class: observation_class(session, snapshot),
                    isolated_class: observation_class(session, snapshot),
                    confirm_delivery: true,
                    record_touch: true,
                    instrument: session
                        .project()
                        .native_sources()
                        .iter()
                        .any(|source| source.instrument().id() == device.id()),
                });
                for _ in 0..4 {
                    if sink.take_plugin_parameter_observation().is_none() {
                        break;
                    }
                }
                return Err(SessionError::NativeAudio(reason));
            }
            return Ok(None);
        }
        let lost_path = if matches!(result, Err(PluginParameterGestureError::SourceLost(_))) {
            self.state.active.as_mut().map(|active| {
                active.source_lost = true;
                active.path.clone()
            })
        } else {
            None
        };
        self.drive(session, context, sink);
        result.map_err(|error| match (error, lost_path) {
            (PluginParameterGestureError::SourceLost(reason), Some(path)) => {
                SessionError::PluginSourceLost(native_automation_recording::retained_error(
                    &path, &reason,
                ))
            }
            (error, _) => host_error(error),
        })?;
        if matches!(
            operation,
            PluginGesturePreparation::Commit(_) | PluginGesturePreparation::Restore { .. }
        ) {
            self.finish(session, operation, context, sink, current, admission)
        } else {
            Ok(None)
        }
    }

    pub(super) fn drive(
        &mut self,
        session: &Session,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
    ) {
        if self
            .state
            .active
            .as_ref()
            .is_some_and(|active| matches!(active.outcome, Some(Err(_))))
        {
            self.retain_failed_observations(context, sink);
            self.retire_detached(context);
            return;
        }
        let Some(active) = self.state.active.as_mut() else {
            return;
        };
        // The sink bounds its queue. Limit control work even when callers poll
        // during a UI publication stall; do not pop beyond a blocked original.
        for _ in 0..4 {
            if active.pending.is_none() && !active.sealing {
                active.pending = sink.take_plugin_parameter_observation().map(|observation| {
                    DeviceAutomationTakeAction::PluginObservation(Box::new(observation))
                });
            }
            let Some(action) = active.pending.as_ref() else {
                break;
            };
            active.has_points |= matches!(action, DeviceAutomationTakeAction::PluginObservation(observation)
                if matches!(**observation, PluginParameterObservation::Delivered(_)));
            match self
                .lane
                .receive(session, self.document_base, context, active.scope, action)
            {
                Ok(Step::Progress(DeviceAutomationTakeProgress::Backpressured)) => break,
                Ok(Step::Progress(progress)) => {
                    if let DeviceAutomationTakeAction::PluginObservation(observation) = action {
                        active.released |=
                            matches!(**observation, PluginParameterObservation::Restored(_));
                        active.interrupted |= matches!(
                            **observation,
                            PluginParameterObservation::Interrupted { .. }
                        );
                        active.source_lost |= matches!(
                            **observation,
                            PluginParameterObservation::Interrupted { .. }
                        );
                        active.ended |= matches!(
                            **observation,
                            PluginParameterObservation::Restored(_)
                                | PluginParameterObservation::Unchanged { .. }
                                | PluginParameterObservation::Interrupted { .. }
                        );
                    }
                    active.sealing |= progress == DeviceAutomationTakeProgress::Sealing;
                    active.pending = None;
                }
                Ok(Step::Commit(_, _)) => {
                    active.outcome = Some(Err(
                        "unexpected take adoption during receipt admission".into()
                    ));
                    break;
                }
                Err(reason) => {
                    let _ = self.lane.receive(
                        session,
                        self.document_base,
                        context,
                        active.scope,
                        &DeviceAutomationTakeAction::Interrupt,
                    );
                    active.sealing = true;
                    active.outcome = Some(Err(reason));
                    break;
                }
            }
        }
        // Poll writer ACKs even while the pointer is stationary. A failed
        // append must release its loans and report interruption without waiting
        // for another SDK point; SDK release still has independent custody.
        if active.outcome.is_none() {
            match self.lane.receive(
                session,
                self.document_base,
                context,
                active.scope,
                &DeviceAutomationTakeAction::Poll,
            ) {
                Ok(Step::Progress(DeviceAutomationTakeProgress::Pending)) => {}
                Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(path)))
                    if !active.ended =>
                {
                    active.sealing = true;
                    active.outcome = Some(Err(native_automation_recording::retained_error(
                        &path,
                        "vst3 take writer ended before its SDK terminal",
                    )));
                }
                outcome => active.outcome = Some(outcome),
            }
        }
        if active.outcome.is_none()
            && active
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            let _ = self.lane.receive(
                session,
                self.document_base,
                context,
                active.scope,
                &DeviceAutomationTakeAction::Interrupt,
            );
            active.outcome = Some(Err(
                "vst3 automation finish timed out; original take remains in recovery custody"
                    .into(),
            ));
        }
        self.retire_detached(context);
    }

    fn retire_detached(&mut self, context: hex_engine::AudioSourceContext) {
        if self.state.active.as_ref().is_some_and(|active| {
            self.state.terminal.is_none()
                && (active.source_lost || context != active.scope.context())
                && active.ended
                && active.outcome.is_some()
        }) && let Some(active) = self.state.active.take()
        {
            self.state.retired = Some(active.key);
            // The original terminal is still owed an exact acknowledgement.
            // Keep it until its synthetic attempt, even after source custody ends.
            if active.pending.is_some()
                || matches!(active.outcome, Some(Err(_)))
                || active.failed_observations.iter().any(Option::is_some)
                || self.lane.busy()
            {
                self.state.failed = Some(active);
            }
            // Otherwise all original receipts are in the sealed recovery file.
            // No SDK/controller custody or canonical edit survives this source.
        }
    }

    fn retain_failed_observations(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
    ) {
        let Some(active) = self.state.active.as_mut() else {
            return;
        };
        for slot in &mut active.failed_observations {
            if slot.is_some() {
                continue;
            }
            let Some(observation) = sink.take_plugin_parameter_observation() else {
                break;
            };
            let matches = |point: PluginParameterDelivery| {
                DeviceAutomationTakeScope::from_plugin_ready(
                    active.scope.context(),
                    &PluginParameterObservation::Ready {
                        key: point.key,
                        context: point.context,
                        target: point.target,
                        source: point.source,
                    },
                )
                .is_ok_and(|scope| scope == active.scope)
                    && point.source.section_loop == active.source.section_loop
                    && point.application.binding == point.target.binding
                    && point.application.parameter == point.target.parameter
                    && point.delivery_performance_frame
                        >= point.application.origin.performance_frame
                    && point.application.value == automation_draft::Vst3ApplicationValue::Restored
            };
            match observation {
                PluginParameterObservation::Restored(point) if matches(point) => {
                    active.released = true;
                    active.ended = true;
                }
                PluginParameterObservation::Unchanged { key }
                    if key == active.key && !active.has_points =>
                {
                    active.ended = true;
                }
                PluginParameterObservation::Interrupted {
                    key,
                    context,
                    target,
                    source,
                    ..
                } if DeviceAutomationTakeScope::from_plugin_ready(
                    active.scope.context(),
                    &PluginParameterObservation::Ready {
                        key,
                        context,
                        target,
                        source,
                    },
                )
                .is_ok_and(|scope| scope == active.scope) =>
                {
                    active.interrupted = true;
                    active.source_lost = true;
                    active.ended = true;
                }
                PluginParameterObservation::Delivered(_) => {
                    active.has_points = true;
                }
                _ => {}
            }
            *slot = Some(observation);
        }
    }

    #[allow(clippy::too_many_arguments)] // Exact terminal and current publication owners.
    fn finish(
        &mut self,
        session: &mut Session,
        operation: PluginGesturePreparation,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        current: &mut Option<resident_audio::Snapshot>,
        admission: sampler_export::Admission<'_>,
    ) -> PreparedGesture {
        let Some(active) = self.state.active.as_mut() else {
            return Err(SessionError::Invalid("vst3 recording is no longer active"));
        };
        if active.outcome.is_none() {
            return self.pending_result();
        }
        let Some(mut active) = self.state.active.take() else {
            return Err(SessionError::Invalid("vst3 recording is no longer active"));
        };
        self.state.terminal = None;
        let terminal = snapshot(operation);
        let result = (|| {
            let outcome = active
                .outcome
                .take()
                .ok_or(SessionError::Invalid("vst3 take completion is unavailable"))?
                .map_err(SessionError::NativeAudio)?;
            if context != active.scope.context() {
                let path = match &outcome {
                    Step::Commit(_, path)
                    | Step::Progress(DeviceAutomationTakeProgress::Retained(path)) => Some(path),
                    _ => None,
                };
                return Err(SessionError::NativeAudio(path.map_or_else(
                    || "vst3 recording source changed".into(),
                    |path| {
                        native_automation_recording::retained_error(
                            path,
                            "vst3 recording source changed",
                        )
                    },
                )));
            }
            if let Step::Progress(DeviceAutomationTakeProgress::Retained(ref path)) = outcome
                && (active.interrupted || context != active.scope.context())
            {
                return Err(SessionError::NativeAudio(
                    native_automation_recording::retained_error(
                        path,
                        "vst3 recording was interrupted before canonical adoption",
                    ),
                ));
            }
            if let Step::Commit(command, path) = outcome
                && matches!(operation, PluginGesturePreparation::Commit(_))
            {
                let next = context
                    .editor_revision
                    .checked_add(1)
                    .ok_or(SessionError::Invalid("preview revision exhausted"))?;
                let (_, snapshot) = session
                    .device_take_resident_snapshot(
                        *command,
                        active.scope.target.into(),
                        hex_engine::AudioSourceContext {
                            editor_revision: next,
                            ..context
                        },
                        sink,
                        admission,
                        current.as_ref(),
                    )
                    .map_err(|error| {
                        let reason =
                            native_automation_recording::retained_error(&path, &error.to_string());
                        if matches!(error, SessionError::PluginSourceLost(_)) {
                            SessionError::PluginSourceLost(reason)
                        } else {
                            SessionError::NativeAudio(reason)
                        }
                    })?;
                *current = Some(snapshot);
            }
            let prepared = current.as_ref().ok_or(SessionError::Invalid(
                "canonical plugin playback is unavailable",
            ))?;
            Ok(Some((
                Arc::new(session.project().clone()),
                Arc::clone(&prepared.prepared),
            )))
        })();
        if matches!(operation, PluginGesturePreparation::Commit(_)) {
            sink.finish_plugin_parameter_commit(
                active.scope.context(),
                terminal.key,
                terminal.sequence,
                result.is_ok(),
            );
        }
        let result = result.map_err(|error| active.terminal_error(error));
        if active.pending.is_some()
            || self.lane.busy()
            || active.failed_observations.iter().any(Option::is_some)
        {
            self.state.failed = Some(active);
        }
        result
    }
}
fn host_error(error: PluginParameterGestureError) -> SessionError {
    match error {
        PluginParameterGestureError::Refused(reason) => SessionError::NativeAudio(reason),
        PluginParameterGestureError::SourceLost(reason) => SessionError::PluginSourceLost(reason),
    }
}
fn observation_class(session: &Session, snapshot: GestureSnapshot) -> Option<[u8; 16]> {
    let crate::device_gesture::GestureTarget::PluginParameter { device, parameter } =
        snapshot.target
    else {
        return None;
    };
    session
        .project()
        .device(device)?
        .automation_target(hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter })?
        .vst3_class
}

#[cfg(test)]
mod tests;
