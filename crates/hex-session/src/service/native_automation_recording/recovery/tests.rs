use super::*;
use crate::service::native_automation_recording::tests::fixture;
use hex_project::{AutomationValue, Command};
use hex_storage::automation_draft::{
    AppliedEvent, CaptureClock, DraftAbortReason, DraftClock, DraftTerminal, DraftWriter,
};

fn target(scope: DeviceAutomationTakeScope) -> DraftTarget {
    match scope.target {
        DeviceCaptureTarget::Native(target) => DraftTarget::Device {
            device: target.device(),
            target: target.target(),
        },
        DeviceCaptureTarget::Vst3(target) => DraftTarget::Device {
            device: target.device,
            target: hex_project::DeviceAutomationTarget::Vst3 {
                parameter: target.parameter,
            },
        },
    }
}
fn recover(
    session: &Session,
    scope: DeviceAutomationTakeScope,
    path: &Path,
) -> Result<(Box<Command>, TakePublication), String> {
    recover_sealed(session, None, target(scope), scope.stamp, path)
}
fn write(
    path: &Path,
    session: &Session,
    scope: DeviceAutomationTakeScope,
    terminal: DraftTerminal,
) {
    let (device, target) = match scope.target {
        DeviceCaptureTarget::Native(target) => (target.device(), target.target()),
        DeviceCaptureTarget::Vst3(target) => (
            target.device,
            hex_project::DeviceAutomationTarget::Vst3 {
                parameter: target.parameter,
            },
        ),
    };
    let mut writer = DraftWriter::create_with_clock(
        path,
        session.project(),
        DraftTarget::Device { device, target },
        scope.stamp,
        CaptureClock {
            sample_rate: 48_000,
            loop_region: None,
        },
    )
    .unwrap();
    writer
        .append(&[AppliedEvent {
            sequence: 1,
            performance_frame: 12,
            content_frame: 4,
            loop_pass: 0,
            value: AutomationValue::new(0.75).unwrap(),
        }])
        .unwrap();
    writer.finish(terminal).unwrap();
}
fn end() -> DraftTerminal {
    DraftTerminal::Finished(DraftClock {
        performance_frame: 24,
        content_frame: 16,
        loop_pass: 0,
    })
}

#[test]
fn native_and_foreign_sealed_recovery_prepare_one_history_edit_without_changing_original() {
    for native in [false, true] {
        let (files, mut session, mut scope, _) = fixture();
        if native {
            scope.target = DeviceCaptureTarget::Native(
                hex_engine::native_parameter_input_target(
                    session.project(),
                    session.project().main_devices()[0].id(),
                    hex_project::DeviceAutomationTarget::HostOutputGain,
                )
                .unwrap(),
            );
        }
        let path = files.0.join("sealed.hex-automation-draft");
        write(&path, &session, scope, end());
        let original = std::fs::read(&path).unwrap();
        let before = session.project().clone();
        let (command, _) = recover(&session, scope, &path).unwrap();
        session.editor.execute(*command).unwrap();
        let after = session.project().clone();
        assert_ne!(after, before);
        assert!(
            recover(&session, scope, &path).is_err(),
            "source changed after adoption"
        );
        assert!(session.editor.undo());
        assert_eq!(session.project(), &before);
        assert!(session.editor.redo());
        assert_eq!(session.project(), &after);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}

#[test]
fn recovery_refuses_wrong_stamp_class_target_and_source_without_mutating_file_or_project() {
    let (files, mut session, scope, _) = fixture();
    let path = files.0.join("sealed.hex-automation-draft");
    write(&path, &session, scope, end());
    let original = std::fs::read(&path).unwrap();
    let before = session.project().clone();
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        panic!("foreign");
    };
    for wrong in [
        DeviceAutomationTakeScope {
            stamp: DraftProvenance {
                gesture_serial: 2,
                ..scope.stamp
            },
            ..scope
        },
        DeviceAutomationTakeScope {
            target: DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
                parameter: 1,
                ..target
            }),
            ..scope
        },
    ] {
        assert!(recover(&session, wrong, &path).is_err());
        assert_eq!(session.project(), &before);
    }
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device: target.device,
            state: hex_plugin_state::encode(hex_plugin_state::State {
                identity: hex_plugin_state::Identity {
                    class: hex_plugin_state::ClassId::new([8; 16]).unwrap(),
                    binary: hex_plugin_state::BinaryIdentity {
                        executable_sha256: [6; 32],
                        info_sha256: [7; 32],
                    },
                },
                component: Some(&[42; 12]),
                controller: Some(&[]),
            })
            .unwrap(),
        })
        .unwrap();
    let changed = session.project().clone();
    assert!(recover(&session, scope, &path).is_err());
    assert_eq!(session.project(), &changed);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn missing_damaged_cancelled_and_interrupted_takes_are_not_adopted() {
    let (files, session, scope, _) = fixture();
    let before = session.project().clone();
    assert!(recover(&session, scope, &files.0.join("missing")).is_err());
    for (index, terminal) in [
        DraftTerminal::Interrupted,
        DraftTerminal::Aborted(DraftAbortReason::Cancelled),
        end(),
    ]
    .into_iter()
    .enumerate()
    {
        let path = files.0.join(format!("take-{index}.hex-automation-draft"));
        write(&path, &session, scope, terminal);
        if index == 2 {
            let mut bytes = std::fs::read(&path).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            std::fs::write(&path, bytes).unwrap();
        }
        let original = std::fs::read(&path).unwrap();
        assert!(recover(&session, scope, &path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert_eq!(session.project(), &before);
    }
}
