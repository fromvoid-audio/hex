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
fn active(scope: DeviceAutomationTakeScope) -> State {
    State {
        active: Some(Active {
            key: gesture(scope).key,
            scope,
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
fn original_points_and_restoration_reach_the_shared_writer_before_terminal_is_ready() {
    let (files, session, scope, source) = fixture();
    let before = session.project().clone();
    let mut lane = Lane::default();
    lane.begin(&session, None, scope, source, &files.0).unwrap();
    let mut state = active(scope);
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
    let mut state = active(scope);
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
    let mut state = active(scope);
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
