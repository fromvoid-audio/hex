//! Real writer coordination with copied model fixtures; these tests grant no
//! live SDK authority. The desktop signed fixture owns actual PCM admission.
use super::*;
use crate::device_gesture::{GestureKey, GestureTarget, PluginParameterValue};
use crate::service::automation_draft::Vst3ApplicationValue;
use crate::service::native_automation_recording::tests::{application, fixture};
use std::collections::VecDeque;

#[derive(Default)]
struct Sink {
    observations: VecDeque<PluginParameterObservation>,
    restorations: usize,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Err("model fixture has no output".into())
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {
        panic!("no publication in writer fixture");
    }
    fn take_plugin_parameter_observation(&mut self) -> Option<PluginParameterObservation> {
        self.observations.pop_front()
    }
    fn prepare_plugin_parameter_gesture(
        &mut self,
        input: PluginParameterGestureInput<'_>,
    ) -> Result<PluginParameterGestureResult, PluginParameterGestureError> {
        assert!(matches!(
            input.operation(),
            PluginGesturePreparation::Restore { .. }
        ));
        self.restorations += 1;
        let key = snapshot(input.operation()).key;
        self.observations
            .push_back(PluginParameterObservation::Unchanged { key });
        Ok(PluginParameterGestureResult::Applied)
    }
}
fn gesture(scope: DeviceAutomationTakeScope) -> GestureSnapshot {
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    GestureSnapshot {
        key: GestureKey {
            generation: scope.stamp.project_generation,
            token: scope.stamp.gesture_serial,
        },
        target: GestureTarget::PluginParameter {
            device: target.device,
            parameter: target.parameter,
        },
        sequence: 0,
        value: hex_lattice_contract::ParameterValue::Float(0.0),
        secondary_value: None,
        parameters: None,
        creation: None,
        manual_edit: false,
        touched_parameters: Default::default(),
        track_gains: None,
        plugin_value: Some(PluginParameterValue::new(0.25).unwrap()),
    }
}
fn delivery(
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    frame: u64,
    value: Vst3ApplicationValue,
) -> PluginParameterDelivery {
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    PluginParameterDelivery {
        key: gesture(scope).key,
        context: scope.context(),
        target,
        source,
        sequence: frame,
        application: application(scope, frame, value),
        delivery_performance_frame: frame + 100,
    }
}
fn active(scope: DeviceAutomationTakeScope, source: DeviceAutomationSource) -> State {
    State {
        active: Some(Active {
            key: gesture(scope).key,
            scope,
            source,
            has_points: false,
            path: PathBuf::from("model-draft"),
            source_lost: false,
            pending: None,
            released: false,
            ended: false,
            failed_observations: [None; 2],
            interrupted: false,
            sealing: false,
            outcome: None,
            deadline: None,
        }),
        ..State::default()
    }
}

#[test]
fn generic_sealed_recovery_refuses_each_original_custody_owner() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;
    use hex_storage::automation_draft::DraftTerminal;

    let (files, session, scope, source) = fixture();
    let before = session.project().clone();
    let idle = Lane::default();
    assert!(generic_sealed_recovery_idle(&State::default(), &idle));
    let mut state = active(scope, source);
    assert!(!generic_sealed_recovery_idle(&state, &idle));
    let failed = state.active.take().unwrap();
    state.failed = Some(failed);
    assert!(!generic_sealed_recovery_idle(&state, &idle));
    let original_path = files.0.join("sealed-guard-original");
    let lane = failed_lane_for_test(
        &session,
        scope,
        source,
        &original_path,
        DraftTerminal::Interrupted,
    );
    let original = std::fs::read(&original_path).unwrap();
    assert!(!generic_sealed_recovery_idle(&State::default(), &lane));
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    assert_eq!(session.project(), &before);
}

#[test]
fn failed_recovery_snapshots_only_quiescent_original_observations() {
    let (_files, _session, scope, source) = fixture();
    let mut state = active(scope, source);
    assert!(state.failed_observations(scope, source).is_err());
    let mut failed = state.active.take().unwrap();
    failed.pending = Some(DeviceAutomationTakeAction::PluginObservation(Box::new(
        PluginParameterObservation::Delivered(delivery(
            scope,
            source,
            8,
            Vst3ApplicationValue::Value(0.5),
        )),
    )));
    failed.failed_observations = [
        Some(PluginParameterObservation::Delivered(delivery(
            scope,
            source,
            16,
            Vst3ApplicationValue::Value(0.75),
        ))),
        Some(PluginParameterObservation::Restored(delivery(
            scope,
            source,
            24,
            Vst3ApplicationValue::Restored,
        ))),
    ];
    state.failed = Some(failed);
    assert!(state.failed_observations(scope, source).is_err());
    state.failed.as_mut().unwrap().ended = true;
    for _ in 0..2 {
        let observations = state.failed_observations(scope, source).unwrap();
        assert!(
            matches!(observations.as_slice(), [PluginParameterObservation::Delivered(a), PluginParameterObservation::Delivered(b), PluginParameterObservation::Restored(c)] if a.sequence == 8 && b.sequence == 16 && c.sequence == 24)
        );
        assert!(state.failed.as_ref().unwrap().pending.is_some());
        assert!(
            state
                .failed
                .as_ref()
                .unwrap()
                .failed_observations
                .iter()
                .all(Option::is_some)
        );
    }
    let mut wrong_scope = scope;
    wrong_scope.stamp.gesture_serial += 1;
    assert!(state.failed_observations(wrong_scope, source).is_err());
    let mut wrong_source = source;
    wrong_source.transport_epoch += 1;
    assert!(state.failed_observations(scope, wrong_source).is_err());
    state.failed.as_mut().unwrap().failed_observations[0] = None;
    assert!(state.failed_observations(scope, source).is_err());
    state.failed.as_mut().unwrap().failed_observations = [None, None];
    state.failed.as_mut().unwrap().pending = Some(DeviceAutomationTakeAction::Interrupt);
    assert!(state.failed_observations(scope, source).is_err());
    state.failed.as_mut().unwrap().pending = None;
    assert!(state.failed_observations(scope, source).unwrap().is_empty());
    assert!(state.failed.is_some());
}

#[test]
fn failed_custody_query_preserves_original_identity_and_rejects_mismatch() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;
    use DeviceAutomationTakeProgress as Progress;

    let (files, session, scope, source) = fixture();
    let path = files.0.join("discovery-original");
    let lane = failed_lane_for_test(
        &session,
        scope,
        source,
        &path,
        hex_storage::automation_draft::DraftTerminal::Interrupted,
    );
    let bytes = std::fs::read(&path).unwrap();
    let mut state = active(scope, source);
    assert!(!state.close_safe());
    assert!(!lane.close_safe());
    assert_eq!(
        failed_custody(&state, &lane, false).unwrap(),
        Progress::FailedPluginPending
    );
    let mut failed = state.active.take().unwrap();
    failed.ended = true;
    failed.path = path.clone();
    state.failed = Some(failed);
    for _ in 0..2 {
        assert_eq!(
            failed_custody(&state, &lane, false).unwrap(),
            Progress::FailedPluginCustody {
                scope: Box::new(scope),
                path: path.clone()
            }
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(state.failed.is_some());
    }
    assert_eq!(
        failed_custody(&state, &lane, true).unwrap(),
        Progress::FailedPluginPending
    );
    state.failed.as_mut().unwrap().source.transport_epoch += 1;
    assert!(failed_custody(&state, &lane, false).is_err());
    assert!(!state.close_safe());
    assert!(!lane.close_safe());
    state.failed.as_mut().unwrap().source = source;
    state.failed.as_mut().unwrap().path = files.0.join("other");
    assert!(failed_custody(&state, &lane, false).is_err());
    let empty = Lane::default();
    assert_eq!(
        failed_custody(&state, &empty, false).unwrap(),
        Progress::FailedPluginPending
    );
    state.failed = None;
    assert!(state.close_safe());
    assert!(empty.close_safe());
    let pending = GestureRequest::CommitPlugin {
        key: gesture(scope).key,
        sequence: 1,
        value: PluginParameterValue::new(0.25).unwrap(),
    };
    state.defer(pending);
    assert!(!state.close_safe());
    state.attempt(pending);
    assert!(state.close_safe());
    assert_eq!(
        failed_custody(&state, &empty, false).unwrap(),
        Progress::FailedPluginUnavailable
    );
}

#[test]
fn failed_recovery_dispatch_copies_receipts_without_releasing_custody() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;
    use hex_storage::automation_draft::{DraftOutcome, DraftReadLimits, DraftReader};

    let (files, mut session, scope, source) = fixture();
    let original_path = files.0.join("worker-original");
    let mut lane = failed_lane_for_test(
        &session,
        scope,
        source,
        &original_path,
        hex_storage::automation_draft::DraftTerminal::Interrupted,
    );
    let original = std::fs::read(&original_path).unwrap();
    let mut state = active(scope, source);
    let mut failed = state.active.take().unwrap();
    failed.ended = true;
    failed.pending = Some(DeviceAutomationTakeAction::PluginObservation(Box::new(
        PluginParameterObservation::Delivered(delivery(
            scope,
            source,
            8,
            Vst3ApplicationValue::Value(0.0),
        )),
    )));
    let delivered = delivery(scope, source, 16, Vst3ApplicationValue::Value(0.75));
    let mut restored = delivery(scope, source, 24, Vst3ApplicationValue::Restored);
    restored.sequence = delivered.sequence;
    failed.failed_observations = [
        Some(PluginParameterObservation::Delivered(delivered)),
        Some(PluginParameterObservation::Restored(restored)),
    ];
    state.failed = Some(failed);
    let output = files.0.join("worker-copy");
    assert!(start_failed_copy(&state, &mut lane, scope, &output, 3, true).is_err());
    assert!(matches!(
        start_failed_copy(&state, &mut lane, scope, &output, 3, false).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Pending)
    ));
    assert!(!lane.close_safe());
    assert!(!state.close_safe());
    assert!(inspect_failed_copy(&state, &lane, scope, false).is_err());
    assert!(start_failed_copy(&state, &mut lane, scope, &output, 3, false).is_err());
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match lane
            .receive(
                &session,
                None,
                scope.context(),
                scope,
                &DeviceAutomationTakeAction::PollFailedCopy,
            )
            .unwrap()
        {
            Step::Progress(DeviceAutomationTakeProgress::Pending) => {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy { path, receipt }) => {
                assert_eq!(path, output);
                assert_eq!(receipt.events(), 3);
                receipt
                    .verify_on_control(
                        &path,
                        DraftReadLimits {
                            max_source_bytes: 65536,
                        },
                        3,
                    )
                    .unwrap();
                break;
            }
            _ => panic!("unexpected recovery result"),
        }
    }
    assert!(
        matches!(inspect_failed_copy(&state, &lane, scope, false).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy { path, .. }) if path == output)
    );
    assert!(inspect_failed_copy(&state, &lane, scope, true).is_err());
    state.active = active(scope, source).active;
    assert!(inspect_failed_copy(&state, &lane, scope, false).is_err());
    state.active = None;
    let mut wrong_scope = scope;
    wrong_scope.stamp.gesture_serial += 1;
    assert!(inspect_failed_copy(&state, &lane, wrong_scope, false).is_err());
    let failed = state.failed.as_mut().unwrap();
    let saved = failed.failed_observations[1].take();
    assert!(inspect_failed_copy(&state, &lane, scope, false).is_err());
    state.failed.as_mut().unwrap().failed_observations[1] = saved;
    state.failed.as_mut().unwrap().source.transport_epoch += 1;
    assert!(inspect_failed_copy(&state, &lane, scope, false).is_err());
    state.failed.as_mut().unwrap().source = source;
    let Some(DeviceAutomationTakeAction::PluginObservation(observation)) =
        state.failed.as_mut().unwrap().pending.as_mut()
    else {
        panic!("pending observation");
    };
    let PluginParameterObservation::Delivered(delivery) = observation.as_mut() else {
        panic!("delivery");
    };
    delivery.application.value = Vst3ApplicationValue::Value(-0.0);
    assert!(inspect_failed_copy(&state, &lane, scope, false).is_err());
    let Some(DeviceAutomationTakeAction::PluginObservation(observation)) =
        state.failed.as_mut().unwrap().pending.as_mut()
    else {
        panic!("pending observation");
    };
    let PluginParameterObservation::Delivered(delivery) = observation.as_mut() else {
        panic!("delivery");
    };
    delivery.application.value = Vst3ApplicationValue::Value(0.0);
    assert!(
        matches!(inspect_failed_copy(&state, &lane, scope, false).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy { path, .. }) if path == output)
    );
    let reader = DraftReader::open(
        &output,
        DraftReadLimits {
            max_source_bytes: 65536,
        },
    )
    .unwrap();
    assert_eq!(reader.progress().durable_events, 3);
    assert_eq!(
        reader.outcome(),
        DraftOutcome::Finished(restored.application.origin)
    );
    drop(reader);
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    let failed = state.failed.as_ref().unwrap();
    assert!(failed.pending.is_some());
    assert!(failed.failed_observations.iter().all(Option::is_some));
    assert!(lane.begin(&session, None, scope, source, &files.0).is_err());
    let before = session.project().clone();
    let mut wrong_source = source;
    wrong_source.transport_epoch += 1;
    assert!(
        prepare_failed_adoption(&state, &lane, &session, None, scope, Some(wrong_source), 3)
            .is_err()
    );
    assert!(prepare_failed_adoption(&state, &lane, &session, None, scope, None, 3).is_err());
    assert!(
        prepare_failed_adoption(&state, &lane, &session, None, scope, Some(source), 2).is_err()
    );
    let (command, adoption) =
        prepare_failed_adoption(&state, &lane, &session, None, scope, Some(source), 3).unwrap();
    assert_eq!(session.project(), &before);
    assert!(lane.has_failed_custody());
    assert!(state.failed.is_some());
    for (failed, previous, current, confirmed, expected) in [
        (true, 2, 2, false, RecoveryPublication::Refused),
        (true, 2, 3, true, RecoveryPublication::Refused),
        (false, 2, 2, true, RecoveryPublication::Unchanged),
        (false, 2, 3, false, RecoveryPublication::Refused),
        (false, 2, 4, true, RecoveryPublication::Refused),
        (false, u64::MAX, 0, true, RecoveryPublication::Refused),
    ] {
        let publication = classify_recovery_publication(failed, previous, current, confirmed);
        assert_eq!(publication, expected);
        finish_failed_adoption(&mut state, &mut lane, &adoption, publication).unwrap();
        assert!(lane.has_failed_custody());
        assert!(state.failed.is_some());
        assert_eq!(session.project(), &before);
    }
    session.editor.execute(*command).unwrap();
    let after = session.project().clone();
    assert_ne!(after, before);
    let saved = state.failed.as_mut().unwrap().failed_observations[1].take();
    assert_eq!(
        classify_recovery_publication(false, 2, 3, true),
        RecoveryPublication::Committed
    );
    assert!(
        finish_failed_adoption(
            &mut state,
            &mut lane,
            &adoption,
            RecoveryPublication::Committed
        )
        .is_err()
    );
    assert!(lane.has_failed_custody());
    assert!(state.failed.is_some());
    state.failed.as_mut().unwrap().failed_observations[1] = saved;
    finish_failed_adoption(
        &mut state,
        &mut lane,
        &adoption,
        RecoveryPublication::Committed,
    )
    .unwrap();
    assert!(!lane.has_failed_custody());
    assert!(state.failed.is_none());
    assert_eq!(
        failed_custody(&state, &lane, false).unwrap(),
        DeviceAutomationTakeProgress::FailedPluginUnavailable
    );
    assert!(
        finish_failed_adoption(
            &mut state,
            &mut lane,
            &adoption,
            RecoveryPublication::Committed
        )
        .is_err()
    );
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &after);
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    assert!(output.exists());
}

#[test]
fn failed_recovery_dispatch_preserves_already_admitted_terminal_without_late_receipts() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;
    use hex_storage::automation_draft::{
        DraftClock, DraftOutcome, DraftReadLimits, DraftReader, DraftTerminal,
    };

    let (files, session, scope, source) = fixture();
    let original_path = files.0.join("admitted-original");
    let end = DraftClock {
        performance_frame: 32,
        content_frame: 24,
        loop_pass: 0,
    };
    let mut lane = failed_lane_for_test(
        &session,
        scope,
        source,
        &original_path,
        DraftTerminal::Finished(end),
    );
    let original = std::fs::read(&original_path).unwrap();
    let state = State::default();
    let output = files.0.join("admitted-copy");
    assert!(matches!(
        start_failed_copy(&state, &mut lane, scope, &output, 1, false).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Pending)
    ));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match lane
            .receive(
                &session,
                None,
                scope.context(),
                scope,
                &DeviceAutomationTakeAction::PollFailedCopy,
            )
            .unwrap()
        {
            Step::Progress(DeviceAutomationTakeProgress::Pending) => {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy { path, receipt }) => {
                assert_eq!(path, output);
                assert_eq!(receipt.events(), 1);
                break;
            }
            _ => panic!("unexpected recovery result"),
        }
    }
    let reader = DraftReader::open(
        &output,
        DraftReadLimits {
            max_source_bytes: 65536,
        },
    )
    .unwrap();
    assert_eq!(reader.outcome(), DraftOutcome::Finished(end));
    assert_eq!(reader.progress().durable_events, 1);
    assert_eq!(std::fs::read(original_path).unwrap(), original);
    assert!(lane.begin(&session, None, scope, source, &files.0).is_err());
}
#[test]
fn original_points_and_restoration_reach_the_shared_writer_before_terminal_is_ready() {
    let (files, session, scope, source) = fixture();
    let before = session.project().clone();
    let mut lane = Lane::default();
    lane.begin(&session, None, scope, source, &files.0).unwrap();
    let mut state = active(scope, source);
    let request = GestureRequest::CommitPlugin {
        key: gesture(scope).key,
        sequence: 2,
        value: PluginParameterValue::new(0.25).unwrap(),
    };
    let mut sink = Sink::default();
    sink.observations
        .push_back(PluginParameterObservation::Delivered(delivery(
            scope,
            source,
            8,
            Vst3ApplicationValue::Value(0.75),
        )));
    {
        let mut prep = Preparation {
            state: &mut state,
            lane: &mut lane,
            document_base: None,
            recovery: None,
            may_defer: true,
            deferred: false,
        };
        prep.drive(&session, scope.context(), &mut sink);
    }
    assert_eq!(session.project(), &before);
    state.defer(request);
    // A ready retry before SDK release may send the actual Commit, but cannot
    // adopt the writer while it has no confirmed exclusive restoration.
    assert!(state.active.as_ref().unwrap().outcome.is_none());
    sink.observations
        .push_back(PluginParameterObservation::Restored(delivery(
            scope,
            source,
            24,
            Vst3ApplicationValue::Restored,
        )));
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        Preparation {
            state: &mut state,
            lane: &mut lane,
            document_base: None,
            recovery: None,
            may_defer: true,
            deferred: false,
        }
        .drive(&session, scope.context(), &mut sink);
        if state.ready().is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let active = state.active.as_mut().unwrap();
    assert!(active.released && active.ended);
    let Step::Commit(command, path) = active.outcome.take().unwrap().unwrap() else {
        panic!("adoption");
    };
    assert!(path.is_file());
    let mut session = session;
    session.editor.execute(*command).unwrap();
    assert_ne!(session.project(), &before);
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
}
#[test]
fn failed_writer_retains_late_point_and_actual_terminal_without_fabricating_release() {
    let (_files, session, scope, source) = fixture();
    let mut state = active(scope, source);
    let active = state.active.as_mut().unwrap();
    active.outcome = Some(Err("disk failure".into()));
    let point = delivery(scope, source, 8, Vst3ApplicationValue::Value(0.75));
    let end = delivery(scope, source, 24, Vst3ApplicationValue::Restored);
    let mut sink = Sink {
        observations: VecDeque::from([
            PluginParameterObservation::Delivered(point),
            PluginParameterObservation::Restored(end),
        ]),
        ..Sink::default()
    };
    let mut lane = Lane::default();
    Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    }
    .drive(&session, scope.context(), &mut sink);
    let active = state.active.as_ref().unwrap();
    assert!(active.released && active.ended);
    assert!(
        matches!(active.failed_observations[0], Some(PluginParameterObservation::Delivered(retained)) if retained.application == point.application)
    );
    assert!(
        matches!(active.failed_observations[1], Some(PluginParameterObservation::Restored(retained)) if retained.application == end.application)
    );
    assert!(sink.observations.is_empty());
    assert_eq!(sink.restorations, 0);
}
#[test]
fn another_sources_terminal_does_not_prove_this_recording_released() {
    let (_files, session, scope, source) = fixture();
    let mut state = active(scope, source);
    state.active.as_mut().unwrap().outcome = Some(Err("disk failure".into()));
    let mut end = delivery(scope, source, 24, Vst3ApplicationValue::Restored);
    end.key.token += 1;
    let mut sink = Sink {
        observations: VecDeque::from([PluginParameterObservation::Restored(end)]),
        ..Sink::default()
    };
    let mut lane = Lane::default();
    Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    }
    .drive(&session, scope.context(), &mut sink);
    assert!(!state.active.as_ref().unwrap().released);
    assert!(!state.active.as_ref().unwrap().ended);
    assert!(state.active.as_ref().unwrap().failed_observations[0].is_some());
}

#[test]
fn failed_admission_releases_the_empty_sdk_gesture_before_any_point() {
    let (files, mut session, scope, source) = fixture();
    let before = session.project().clone();
    let occupied = files.0.join("occupied");
    std::fs::write(&occupied, b"preserve").unwrap();
    let recovery = occupied.join("recovery.hex");
    let snapshot = gesture(scope);
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    let mut sink = Sink {
        observations: VecDeque::from([PluginParameterObservation::Ready {
            key: snapshot.key,
            context: scope.context(),
            target,
            source,
        }]),
        ..Sink::default()
    };
    let mut state = State::default();
    let mut lane = Lane::default();
    let result = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: Some(&recovery),
        may_defer: false,
        deferred: false,
    }
    .after(
        &mut session,
        PluginGesturePreparation::Begin {
            snapshot,
            recording: true,
            origin: crate::device_gesture::PluginParameterOrigin {
                context: scope.context(),
                device: target.device,
                parameter: target.parameter,
                page_first: 0,
                metadata_revision: 1,
                initial: snapshot.plugin_value.unwrap(),
            },
        },
        scope.context(),
        &mut sink,
        Ok(PluginParameterGestureResult::Applied),
        &mut None,
        sampler_export::Admission::default(),
    );
    assert!(result.is_err());
    assert_eq!(sink.restorations, 1);
    assert!(sink.observations.is_empty());
    assert!(!lane.busy());
    assert!(!state.pending());
    assert_eq!(std::fs::read(occupied).unwrap(), b"preserve");
    assert_eq!(session.project(), &before);
}

#[test]
fn expired_failed_release_refuses_without_discarding_original_or_claiming_restoration() {
    let (_files, mut session, scope, source) = fixture();
    let before = session.project().clone();
    let mut state = active(scope, source);
    let active = state.active.as_mut().unwrap();
    active.has_points = true;
    active.pending = Some(DeviceAutomationTakeAction::PluginObservation(Box::new(
        PluginParameterObservation::Delivered(delivery(
            scope,
            source,
            8,
            Vst3ApplicationValue::Value(0.75),
        )),
    )));
    active.outcome = Some(Err("disk failure".into()));
    active.deadline = Some(Instant::now());
    let mut sink = Sink::default();
    let mut lane = Lane::default();
    let mut prep = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: true,
        deferred: false,
    };
    let snapshot = gesture(scope);
    let result = prep.before(
        &mut session,
        PluginGesturePreparation::Restore {
            snapshot,
            initial: snapshot.plugin_value.unwrap(),
        },
        scope.context(),
        &mut sink,
        &mut None,
        sampler_export::Admission::default(),
    );
    assert!(matches!(
        result,
        Some(Err(SessionError::Invalid(
            "vst3 restoration is still pending; retry release to recover the retained take"
        )))
    ));
    assert!(!prep.deferred);
    let active = state.active.as_ref().unwrap();
    assert!(!active.ended && !active.released);
    assert!(active.pending.is_some());
    assert!(matches!(
        active.failed_observations[0],
        Some(PluginParameterObservation::Unchanged { .. })
    ));
    assert_eq!(session.project(), &before);
}

#[test]
fn stationary_recording_observes_writer_interruption_without_inventing_sdk_end() {
    let (files, session, scope, source) = fixture();
    let before = session.project().clone();
    let mut lane = Lane::default();
    lane.begin(&session, None, scope, source, &files.0).unwrap();
    lane.receive(
        &session,
        None,
        scope.context(),
        scope,
        &DeviceAutomationTakeAction::Interrupt,
    )
    .unwrap();
    let mut state = active(scope, source);
    let mut sink = Sink::default();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        Preparation {
            state: &mut state,
            lane: &mut lane,
            document_base: None,
            recovery: None,
            may_defer: false,
            deferred: false,
        }
        .drive(&session, scope.context(), &mut sink);
        if state.active.as_ref().unwrap().outcome.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let active = state.active.as_ref().unwrap();
    assert!(matches!(active.outcome, Some(Err(_))));
    assert!(!active.ended && !active.released);
    assert_eq!(sink.restorations, 0);
    assert_eq!(session.project(), &before);
    assert!(!lane.busy());
}

#[test]
fn source_loss_drains_original_then_interruption_and_releases_the_completed_writer_owner() {
    let (files, mut session, scope, source) = fixture();
    let before = session.project().clone();
    let mut lane = Lane::default();
    lane.begin(&session, None, scope, source, &files.0).unwrap();
    let path = lane.path().unwrap().to_owned();
    let mut state = active(scope, source);
    state.active.as_mut().unwrap().path = path.clone();
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    let snapshot = gesture(scope);
    let mut sink = Sink {
        observations: VecDeque::from([
            PluginParameterObservation::Delivered(delivery(
                scope,
                source,
                8,
                Vst3ApplicationValue::Value(0.75),
            )),
            PluginParameterObservation::Interrupted {
                key: snapshot.key,
                context: scope.context(),
                target,
                source,
                sequence: 9,
            },
        ]),
        ..Sink::default()
    };
    let result = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    }
    .after(
        &mut session,
        PluginGesturePreparation::Preview(snapshot),
        scope.context(),
        &mut sink,
        Err(PluginParameterGestureError::SourceLost(
            "model output closed".into(),
        )),
        &mut None,
        sampler_export::Admission::default(),
    );
    assert!(
        matches!(result, Err(SessionError::PluginSourceLost(ref reason)) if reason.contains(&path.display().to_string()))
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while state.pending() {
        Preparation {
            state: &mut state,
            lane: &mut lane,
            document_base: None,
            recovery: None,
            may_defer: false,
            deferred: false,
        }
        .drive(&session, scope.context(), &mut sink);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!lane.busy());
    assert!(state.failed.is_none());
    assert!(path.is_file());
    // SDK/output custody ended before a terminal was dispatched. The actor's
    // later barrier must never fall back to ordinary opaque parameter commit.
    let terminal = state.terminal_for(Some(snapshot)).unwrap();
    let mut preparation = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: true,
        deferred: false,
    };
    let operation = PluginGesturePreparation::Commit(snapshot);
    assert!(preparation.handles(operation));
    assert_eq!(terminal.key(), snapshot.key);
    assert!(matches!(
        preparation.before(
            &mut session,
            operation,
            scope.context(),
            &mut sink,
            &mut None,
            sampler_export::Admission::default(),
        ),
        Some(Err(SessionError::PluginSourceLost(_)))
    ));
    let reader = hex_storage::automation_draft::DraftReader::open(
        &path,
        hex_storage::automation_draft::DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap();
    assert_eq!(reader.progress().durable_events, 1);
    assert_eq!(
        reader.outcome(),
        hex_storage::automation_draft::DraftOutcome::Aborted(
            hex_storage::automation_draft::DraftAbortReason::OutputFailed
        )
    );
    assert_eq!(session.project(), &before);
}

#[test]
fn retiring_a_source_preserves_its_deferred_terminal_until_the_exact_attempt() {
    let (_files, mut session, scope, source) = fixture();
    let mut state = active(scope, source);
    let snapshot = gesture(scope);
    let request = GestureRequest::CommitPlugin {
        key: snapshot.key,
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    state.defer(request);
    let active = state.active.as_mut().unwrap();
    active.source_lost = true;
    active.ended = true;
    active.outcome = Some(Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(
        active.path.clone(),
    ))));
    let mut lane = Lane::default();
    let mut sink = Sink::default();
    Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    }
    .drive(&session, scope.context(), &mut sink);
    assert_eq!(
        state
            .ready()
            .map(|pending| (pending.key(), pending.phase(), pending.sequence())),
        Some((request.key(), request.phase(), request.sequence()))
    );
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    let begin = PluginGesturePreparation::Begin {
        snapshot,
        recording: true,
        origin: crate::device_gesture::PluginParameterOrigin {
            context: scope.context(),
            device: target.device,
            parameter: target.parameter,
            page_first: 0,
            metadata_revision: 1,
            initial: snapshot.plugin_value.unwrap(),
        },
    };
    let mut prep = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    };
    assert!(
        prep.before(
            &mut session,
            begin,
            scope.context(),
            &mut sink,
            &mut None,
            sampler_export::Admission::default()
        )
        .is_some(),
        "a new Begin must not overwrite an admitted terminal still awaiting its attempt"
    );
    state.attempt(request);
    assert!(state.ready().is_none());
    Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    }
    .drive(&session, scope.context(), &mut sink);
    assert!(!state.pending());
    let mut prep = Preparation {
        state: &mut state,
        lane: &mut lane,
        document_base: None,
        recovery: None,
        may_defer: false,
        deferred: false,
    };
    assert!(
        prep.before(
            &mut session,
            begin,
            scope.context(),
            &mut sink,
            &mut None,
            sampler_export::Admission::default()
        )
        .is_none()
    );
    assert_eq!(sink.restorations, 0);
}

#[test]
fn cached_interruption_retires_the_exact_gesture_without_claiming_a_retained_preview() {
    use crate::device_gesture::{
        GestureClient, GestureMachine, GesturePreparation, PluginParameterOrigin,
    };
    let (_files, mut session, scope, source) = fixture();
    let snapshot = gesture(scope);
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign fixture");
    };
    let origin = PluginParameterOrigin {
        context: scope.context(),
        device: target.device,
        parameter: target.parameter,
        page_first: 0,
        metadata_revision: 1,
        initial: snapshot.plugin_value.unwrap(),
    };
    let mut state = active(scope, source);
    let active = state.active.as_mut().unwrap();
    active.source_lost = true;
    active.interrupted = true;
    active.ended = true;
    let mut machine = GestureMachine::default();
    let mut client = GestureClient::default();
    client.begin_plugin_recording(origin).unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    let begun = machine.handle_prepared(
        &mut session,
        begin,
        None,
        scope.stamp.project_generation,
        |_, _| Ok(None),
    );
    assert!(begun.ack.accepted);
    assert!(client.acknowledge_with_snapshot(&begun.ack, machine.snapshot()));
    client
        .finish_plugin(PluginParameterValue::new(0.75).unwrap())
        .unwrap();
    let terminal = client.next_request().unwrap();
    client.mark_submitted();
    let ended = machine.handle_prepared(
        &mut session,
        terminal,
        None,
        scope.stamp.project_generation,
        |_, operation| match operation {
            GesturePreparation::Plugin(PluginGesturePreparation::Commit(_)) => Err(active
                .terminal_error(SessionError::NativeAudio(
                    "original recording scope ended; draft retained".into(),
                ))),
            _ => Ok(None),
        },
    );
    assert!(!ended.ack.accepted);
    assert!(
        ended.ack.source_lost,
        "a cached source interruption must not become a retained-preview refusal"
    );
    assert!(!ended.ack.preview_retained);
    assert!(machine.snapshot().is_none());
    assert!(client.acknowledge_with_snapshot(&ended.ack, machine.snapshot()));
    assert!(client.begin_plugin_recording(origin).is_ok());
}
