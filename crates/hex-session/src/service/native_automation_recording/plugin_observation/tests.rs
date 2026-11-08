use super::*;
use crate::device_gesture::GestureKey;
use crate::service::automation_draft::Vst3ApplicationValue;
use crate::service::native_automation_recording::tests::{application, fixture, poll};
use hex_storage::automation_draft::{DraftOutcome, DraftReader};

fn ready(scope: DeviceAutomationTakeScope, source: DeviceAutomationSource) -> Observation {
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign")
    };
    Observation::Ready {
        key: GestureKey {
            generation: scope.stamp.project_generation,
            token: scope.stamp.gesture_serial,
        },
        context: scope.context(),
        target,
        source,
    }
}
fn point(
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    frame: u64,
    value: Vst3ApplicationValue,
) -> PluginParameterDelivery {
    let Observation::Ready {
        key,
        context,
        target,
        ..
    } = ready(scope, source)
    else {
        panic!("ready")
    };
    PluginParameterDelivery {
        key,
        context,
        target,
        source,
        sequence: frame,
        application: application(scope, frame, value),
        delivery_performance_frame: frame + 100,
    }
}
fn send(
    lane: &mut Lane,
    session: &Session,
    scope: DeviceAutomationTakeScope,
    observation: Observation,
) -> Result<Step, String> {
    lane.receive(
        session,
        None,
        scope.context(),
        scope,
        &DeviceAutomationTakeAction::PluginObservation(Box::new(observation)),
    )
}
#[test]
fn observed_points_and_exclusive_restoration_adopt_once_with_original_clocks() {
    let (files, mut session, scope, source) = fixture();
    let admitted = ready(scope, source);
    assert_eq!(
        DeviceAutomationTakeScope::from_plugin_ready(scope.context(), &admitted).unwrap(),
        scope
    );
    let mut lane = Lane::default();
    lane.begin(&session, None, scope, source, &files.0).unwrap();
    let original = point(scope, source, 8, Vst3ApplicationValue::Value(0.375));
    // Rejection retains the same receipt and leaves its sequence available.
    let mut stale = original;
    stale.key.token += 1;
    assert!(send(&mut lane, &session, scope, Observation::Delivered(stale)).is_err());
    stale = original;
    stale.source.transport_epoch += 1;
    assert!(send(&mut lane, &session, scope, Observation::Delivered(stale)).is_err());
    stale = original;
    stale.target.binding.instance += 1;
    assert!(send(&mut lane, &session, scope, Observation::Delivered(stale)).is_err());
    stale = original;
    stale.delivery_performance_frame = 0;
    assert!(send(&mut lane, &session, scope, Observation::Delivered(stale)).is_err());
    assert!(matches!(
        send(&mut lane, &session, scope, Observation::Delivered(original)).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Appended)
    ));
    let key = original.key;
    assert!(send(&mut lane, &session, scope, Observation::Unchanged { key }).is_err());
    assert!(send(&mut lane, &session, scope, Observation::Restored(original)).is_err());
    let end = point(scope, source, 24, Vst3ApplicationValue::Restored);
    assert!(matches!(
        send(&mut lane, &session, scope, Observation::Restored(end)).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Sealing)
    ));
    let Step::Commit(command, path) = poll(&mut lane, &session, scope, scope.context()) else {
        panic!("commit")
    };
    let mut reader = DraftReader::open(
        &path,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap();
    assert_eq!(reader.provenance(), scope.stamp);
    assert_eq!(
        reader.outcome(),
        DraftOutcome::Finished(end.application.origin)
    );
    let mut events = [None; 2];
    assert_eq!(reader.read_events(0, &mut events).unwrap(), 1);
    let event = events[0].unwrap();
    assert_eq!(
        event.performance_frame,
        original.application.origin.performance_frame
    );
    assert_eq!(
        event.content_frame,
        original.application.origin.content_frame
    );
    assert_eq!(event.value.get(), 0.375);
    let before = session.project().clone();
    session.editor.execute(*command).unwrap();
    let after = session.project().clone();
    assert_ne!(before, after);
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&after).unwrap()).unwrap(),
        after
    );
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &after);
}
#[test]
fn empty_and_interrupted_observations_retain_without_a_fabricated_end() {
    for interrupted in [false, true] {
        let (files, session, scope, source) = fixture();
        let before = session.project().clone();
        let mut lane = Lane::default();
        lane.begin(&session, None, scope, source, &files.0).unwrap();
        let Observation::Ready {
            key,
            context,
            target,
            ..
        } = ready(scope, source)
        else {
            panic!("ready")
        };
        let observation = if interrupted {
            let observed = point(scope, source, 8, Vst3ApplicationValue::Value(0.5));
            send(&mut lane, &session, scope, Observation::Delivered(observed)).unwrap();
            Observation::Interrupted {
                key,
                context,
                target,
                source,
                sequence: 9,
            }
        } else {
            Observation::Unchanged { key }
        };
        send(&mut lane, &session, scope, observation).unwrap();
        let Step::Progress(DeviceAutomationTakeProgress::Retained(path)) =
            poll(&mut lane, &session, scope, scope.context())
        else {
            panic!("retained")
        };
        let reader = DraftReader::open(
            &path,
            DraftReadLimits {
                max_source_bytes: 64 * 1024,
            },
        )
        .unwrap();
        assert_eq!(
            reader.outcome(),
            DraftOutcome::Aborted(if interrupted {
                DraftAbortReason::OutputFailed
            } else {
                DraftAbortReason::Cancelled
            })
        );
        assert_eq!(session.project(), &before);
        assert!(!lane.busy());
    }
}
#[test]
fn ready_requires_current_exact_source_and_failed_draft_admission_owns_no_take() {
    let (files, session, scope, source) = fixture();
    let observation = ready(scope, source);
    let stale = AudioSourceContext {
        editor_revision: scope.context().editor_revision + 1,
        ..scope.context()
    };
    assert!(DeviceAutomationTakeScope::from_plugin_ready(stale, &observation).is_err());
    let Observation::Ready {
        key,
        context,
        target,
        ..
    } = observation
    else {
        panic!("ready")
    };
    for observation in [
        Observation::Unchanged { key },
        Observation::Ready {
            key: GestureKey { token: 0, ..key },
            context,
            target,
            source,
        },
        Observation::Ready {
            key,
            context,
            target,
            source: DeviceAutomationSource {
                output_identity: 99,
                ..source
            },
        },
    ] {
        assert!(
            DeviceAutomationTakeScope::from_plugin_ready(scope.context(), &observation).is_err()
        );
    }
    let file = files.0.join("occupied");
    std::fs::write(&file, b"preserve").unwrap();
    let mut lane = Lane::default();
    assert!(lane.begin(&session, None, scope, source, &file).is_err());
    assert!(!lane.busy());
    assert_eq!(std::fs::read(file).unwrap(), b"preserve");
}
