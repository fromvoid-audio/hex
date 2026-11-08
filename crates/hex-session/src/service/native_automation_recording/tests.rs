//! Shared coordinator proof; canonical-only foreign fixture grants no SDK authority.
use super::*;
use crate::service::automation_draft::{
    Vst3ApplicationValue, Vst3CaptureTarget, Vst3ParameterApplication,
};
use hex_project::{Command, Created};
use hex_storage::automation_draft::{DraftClock, DraftOutcome, DraftReader};
use std::time::{Duration, Instant};
pub(in crate::service) struct Files(pub(in crate::service) PathBuf);
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(in crate::service) fn fixture() -> (
    Files,
    Session,
    DeviceAutomationTakeScope,
    DeviceAutomationSource,
) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let files = Files(std::env::temp_dir().join(format!(
        "hex-device-take-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir(&files.0).unwrap();
    let mut session = Session::new(48_000, 8, 4096).unwrap();
    session
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [6; 32],
                info_sha256: [7; 32],
            },
        },
        component: Some(&[42; 12]),
        controller: Some(&[]),
    })
    .unwrap();
    let Created::Device(device) = session
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let target = DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
        device,
        class: [9; 16],
        parameter: 0,
        binding: hex_lattice_contract::ForeignProcessorBinding {
            root: 9,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
    });
    let scope = DeviceAutomationTakeScope {
        target,
        stamp: DraftProvenance {
            sample_rate: 48_000,
            output_identity: 9,
            project_generation: 0,
            editor_revision: 2,
            audio_revision: 7,
            transport_epoch: 1,
            gesture_serial: 1,
        },
    };
    let source = DeviceAutomationSource {
        target,
        sample_rate: 48_000,
        output_identity: 9,
        audio_revision: 7,
        transport_epoch: 1,
        section_loop: None,
    };
    (files, session, scope, source)
}
pub(in crate::service) fn application(
    scope: DeviceAutomationTakeScope,
    frame: u64,
    value: Vst3ApplicationValue,
) -> Vst3ParameterApplication {
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign")
    };
    Vst3ParameterApplication {
        binding: target.binding,
        parameter: target.parameter,
        origin: DraftClock {
            performance_frame: frame + 8,
            content_frame: frame,
            loop_pass: 0,
        },
        value,
    }
}
pub(super) fn poll(
    lane: &mut Lane,
    session: &Session,
    scope: DeviceAutomationTakeScope,
    current: AudioSourceContext,
) -> Step {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let step = lane
            .receive(
                session,
                None,
                current,
                scope,
                &DeviceAutomationTakeAction::Poll,
            )
            .unwrap();
        if !matches!(step, Step::Progress(DeviceAutomationTakeProgress::Pending)) {
            return step;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn failed_lane_retains_original_custody_through_copy_refusal_and_retry() {
    use crate::service::automation_draft::{AppliedEvent, tests::injected_failed_writer};
    let (files, session, scope, source) = fixture();
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("vst3 target")
    };
    let path = files.0.join("failed.hex-automation-draft");
    let writer = hex_storage::automation_draft::DraftWriter::create_with_clock(
        &path,
        session.project(),
        hex_storage::automation_draft::DraftTarget::Device {
            device: target.device,
            target: hex_project::DeviceAutomationTarget::Vst3 {
                parameter: target.parameter,
            },
        },
        scope.stamp,
        CaptureClock {
            sample_rate: 48000,
            loop_region: None,
        },
    )
    .unwrap();
    let event = AppliedEvent {
        sequence: 1,
        performance_frame: 8,
        content_frame: 0,
        loop_pass: 0,
        value: hex_project::AutomationValue::new(0.5).unwrap(),
    };
    let outcome = injected_failed_writer(writer, event);
    assert!(outcome.failure.is_some());
    let original = std::fs::read(&path).unwrap();
    let mut lane = Lane::default();
    lane.failed = Some(Failed {
        scope,
        outcome: Arc::new(outcome),
    });
    let current = scope.context();
    let mut wrong_scope = scope;
    wrong_scope.stamp.gesture_serial += 1;
    let destination = files.0.join("copy.hex-automation-draft");
    std::fs::write(&destination, b"existing").unwrap();
    let start = DeviceAutomationTakeAction::CopyFailed {
        destination: destination.clone(),
        max_events: 1,
    };
    assert!(
        lane.receive(&session, None, current, wrong_scope, &start)
            .is_err()
    );
    assert!(
        lane.receive(
            &session,
            None,
            current,
            scope,
            &DeviceAutomationTakeAction::CopyFailed {
                destination: destination.clone(),
                max_events: hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64 + 1
            }
        )
        .is_err()
    );
    assert!(matches!(
        lane.receive(&session, None, current, scope, &start)
            .unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Pending)
    ));
    assert!(
        lane.receive(&session, None, current, scope, &start)
            .is_err()
    );
    let wait = |lane: &mut Lane| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let result = lane.receive(
                &session,
                None,
                current,
                scope,
                &DeviceAutomationTakeAction::PollFailedCopy,
            );
            if !matches!(
                result,
                Ok(Step::Progress(DeviceAutomationTakeProgress::Pending))
            ) {
                break result;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    assert!(wait(&mut lane).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
    let retry = files.0.join("retry.hex-automation-draft");
    let start = DeviceAutomationTakeAction::CopyFailed {
        destination: retry.clone(),
        max_events: 1,
    };
    lane.receive(&session, None, current, scope, &start)
        .unwrap();
    assert!(
        matches!(wait(&mut lane).unwrap(), Step::Progress(DeviceAutomationTakeProgress::Retained(path)) if path == retry)
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        lane.failed.as_ref().unwrap().outcome.retained[0].events(),
        &[event]
    );
    assert!(lane.begin(&session, None, scope, source, &files.0).is_err());
    let reader = DraftReader::open(
        &retry,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap();
    assert_eq!(reader.outcome(), DraftOutcome::Interrupted);
    assert_eq!(reader.progress().durable_events, 1);
}
#[test]
fn foreign_take_uses_one_worker_and_prepares_one_class_bound_history_edit() {
    let (files, mut session, scope, source) = fixture();
    let mut lane = Lane::default();
    assert!(
        lane.begin(
            &session,
            None,
            scope,
            DeviceAutomationSource {
                transport_epoch: 2,
                ..source
            },
            &files.0
        )
        .is_err()
    );
    assert!(std::fs::read_dir(&files.0).unwrap().next().is_none());
    assert!(matches!(
        lane.begin(&session, None, scope, source, &files.0).unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Started)
    ));
    assert!(lane.begin(&session, None, scope, source, &files.0).is_err());
    let native = hex_engine::native_parameter_input_target(
        session.project(),
        session.project().main_devices()[0].id(),
        hex_project::DeviceAutomationTarget::HostOutputGain,
    )
    .unwrap();
    let native_scope = DeviceAutomationTakeScope {
        target: native.into(),
        ..scope
    };
    assert!(
        lane.begin(
            &session,
            None,
            native_scope,
            DeviceAutomationSource {
                target: native.into(),
                ..source
            },
            &files.0
        )
        .is_err()
    );
    let foreign = application(scope, 8, Vst3ApplicationValue::Value(0.25));
    let batch = Arc::from([(1, foreign)]);
    let stale = DeviceAutomationTakeScope {
        stamp: DraftProvenance {
            gesture_serial: 2,
            ..scope.stamp
        },
        ..scope
    };
    assert!(
        lane.receive(
            &session,
            None,
            scope.context(),
            stale,
            &DeviceAutomationTakeAction::AppendVst3(Arc::clone(&batch))
        )
        .is_err()
    );
    assert!(
        lane.receive(
            &session,
            None,
            scope.context(),
            scope,
            &DeviceAutomationTakeAction::Append(Arc::from([]))
        )
        .is_err()
    );
    assert!(matches!(
        lane.receive(
            &session,
            None,
            scope.context(),
            scope,
            &DeviceAutomationTakeAction::AppendVst3(batch)
        )
        .unwrap(),
        Step::Progress(DeviceAutomationTakeProgress::Appended)
    ));
    lane.receive(
        &session,
        None,
        scope.context(),
        scope,
        &DeviceAutomationTakeAction::FinishVst3(application(
            scope,
            16,
            Vst3ApplicationValue::Restored,
        )),
    )
    .unwrap();
    let Step::Commit(command, path) = poll(&mut lane, &session, scope, scope.context()) else {
        panic!("commit")
    };
    assert!(!lane.busy());
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("target")
    };
    assert!(
        matches!(command.as_ref(),Command::SpliceDeviceAutomationTouch {device,vst3_class:Some(class),..} if *device==target.device && *class==target.class)
    );
    let before = session.project().clone();
    session.editor.execute(*command).unwrap();
    let after = session.project().clone();
    assert_ne!(before, after);
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &after);
    let reopened = hex_storage::decode(&hex_storage::encode(session.project()).unwrap()).unwrap();
    assert_eq!(&reopened, session.project());
    assert!(path.exists());
}
#[test]
fn interrupted_or_replaced_foreign_source_retains_its_original_draft() {
    for interrupt in [false, true] {
        let (files, session, scope, source) = fixture();
        let mut lane = Lane::default();
        lane.begin(&session, None, scope, source, &files.0).unwrap();
        lane.receive(
            &session,
            None,
            scope.context(),
            scope,
            &DeviceAutomationTakeAction::AppendVst3(Arc::from([(
                1,
                application(scope, 8, Vst3ApplicationValue::Value(0.5)),
            )])),
        )
        .unwrap();
        let current = AudioSourceContext {
            editor_revision: 3,
            ..scope.context()
        };
        if interrupt {
            lane.interrupt_if_source_changed(current)
        } else {
            lane.receive(
                &session,
                None,
                scope.context(),
                scope,
                &DeviceAutomationTakeAction::FinishVst3(application(
                    scope,
                    16,
                    Vst3ApplicationValue::Restored,
                )),
            )
            .unwrap();
        }
        let Step::Progress(DeviceAutomationTakeProgress::Retained(path)) =
            poll(&mut lane, &session, scope, current)
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
        assert_eq!(reader.provenance(), scope.stamp);
        assert_eq!(reader.source(), session.project());
        if !interrupt {
            assert!(matches!(reader.outcome(), DraftOutcome::Finished(_)));
        }
        assert!(!lane.busy());
    }
}
