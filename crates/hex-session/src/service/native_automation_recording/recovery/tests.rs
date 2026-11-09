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
    let selected = inspect_sealed_file(path)?;
    recover_sealed(
        session,
        None,
        target(scope),
        scope.stamp,
        path,
        AutomationRecoveryFileIdentity::Sealed(selected.identity),
    )
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

#[test]
fn verified_vst3_recovery_prepares_one_edit_and_refuses_changed_custody() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;

    let (files, mut session, scope, source) = fixture();
    let original_path = files.0.join("failed-original");
    let lane = failed_lane_for_test(&session, scope, source, &original_path, end());
    let failed = lane.failed.as_ref().unwrap();
    let original = std::fs::read(&original_path).unwrap();
    let copy = files.0.join("verified-copy");
    let limits = DraftReadLimits {
        max_source_bytes: 65536,
    };
    let receipt = copy_failed_plugin_with_receipt_on_control(
        &failed.outcome,
        &copy,
        limits,
        2,
        scope,
        source,
        &[],
    )
    .unwrap();
    let before = session.project().clone();
    assert!(
        prepare_verified_vst3_recovery_on_control(&session, None, scope, &copy, receipt, 0)
            .is_err()
    );
    let DeviceCaptureTarget::Vst3(vst3) = scope.target else {
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
                ..vst3
            }),
            ..scope
        },
        DeviceAutomationTakeScope {
            target: DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
                class: [8; 16],
                ..vst3
            }),
            ..scope
        },
    ] {
        assert!(
            prepare_verified_vst3_recovery_on_control(&session, None, wrong, &copy, receipt, 2)
                .is_err()
        );
    }
    let mut changed = std::fs::read(&copy).unwrap();
    changed.push(0);
    let damaged = files.0.join("damaged-copy");
    std::fs::write(&damaged, changed).unwrap();
    assert!(
        prepare_verified_vst3_recovery_on_control(&session, None, scope, &damaged, receipt, 2)
            .is_err()
    );
    let replacement = files.0.join("valid-replacement");
    write(&replacement, &session, scope, end());
    assert!(
        recover_sealed(
            &session,
            None,
            target(scope),
            scope.stamp,
            &replacement,
            AutomationRecoveryFileIdentity::Copy(receipt)
        )
        .is_err()
    );
    assert!(
        recover_sealed(
            &session,
            None,
            target(scope),
            scope.stamp,
            &damaged,
            AutomationRecoveryFileIdentity::Copy(receipt)
        )
        .is_err()
    );
    assert!(
        prepare_verified_vst3_recovery_on_control(&session, None, scope, &replacement, receipt, 2)
            .is_err()
    );
    assert_eq!(session.project(), &before);
    let _command =
        prepare_verified_vst3_recovery_on_control(&session, None, scope, &copy, receipt, 2)
            .unwrap();
    let (verified_command, publication) = recover_sealed(
        &session,
        None,
        target(scope),
        scope.stamp,
        &copy,
        AutomationRecoveryFileIdentity::Copy(receipt),
    )
    .unwrap();
    assert!(matches!(publication, TakePublication::Vst3));
    session.editor.execute(*verified_command).unwrap();
    let after = session.project().clone();
    assert_ne!(after, before);
    assert!(
        prepare_verified_vst3_recovery_on_control(&session, None, scope, &copy, receipt, 2)
            .is_err()
    );
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &after);
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    assert!(lane.failed.is_some());
}

#[test]
fn verified_vst3_recovery_never_prepares_interrupted_or_cancelled_takes() {
    use crate::service::native_automation_recording::tests::failed_lane_for_test;

    let (files, session, scope, source) = fixture();
    let before = session.project().clone();
    for (index, terminal) in [
        DraftTerminal::Interrupted,
        DraftTerminal::Aborted(DraftAbortReason::Cancelled),
    ]
    .into_iter()
    .enumerate()
    {
        let original = files.0.join(format!("failed-{index}"));
        let lane = failed_lane_for_test(&session, scope, source, &original, terminal);
        let copy = files.0.join(format!("copy-{index}"));
        let receipt = copy_failed_plugin_with_receipt_on_control(
            &lane.failed.as_ref().unwrap().outcome,
            &copy,
            DraftReadLimits {
                max_source_bytes: 65536,
            },
            2,
            scope,
            source,
            &[],
        )
        .unwrap();
        assert!(
            prepare_verified_vst3_recovery_on_control(&session, None, scope, &copy, receipt, 2)
                .is_err()
        );
        assert!(lane.failed.is_some());
        assert_eq!(session.project(), &before);
    }
}

#[test]
fn a_verified_native_finished_copy_revalidates_and_adopts_exact_values_once() {
    use crate::service::automation_draft::DeviceDraftInput;
    use hex_engine::{NativeParameterApplication, NativeTransientOperation, NativeTransientValue};
    use hex_lattice_contract::HostGain;

    let (files, mut session, mut scope, mut source) = fixture();
    let target = hex_engine::native_parameter_input_target(
        session.project(),
        session.project().main_devices()[0].id(),
        hex_project::DeviceAutomationTarget::HostOutputGain,
    )
    .unwrap();
    scope.target = target.into();
    source.target = scope.target;
    let path = files.0.join("original-native");
    let (mut input, worker) = DeviceDraftInput::start_on_control(
        &session,
        None,
        path.clone(),
        target,
        scope.stamp,
        source.capture_clock(session.project()),
    )
    .unwrap();
    let applied = NativeParameterApplication {
        identity: target.identity(),
        operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
            HostGain::from_normalized(0.25).unwrap(),
        )),
        arrangement_frame: 4,
        performance_frame: 12,
        loop_pass: 0,
        remaining_latency_frames: 8,
    };
    assert!(
        input
            .append_delivered(target, scope.stamp, &[(1, applied)])
            .unwrap()
            .is_some()
    );
    let interrupted = worker.join_on_control().unwrap();
    let original = std::fs::read(&path).unwrap();
    let copy = files.0.join("native-finished-copy");
    let receipt = copy_native_recovery_on_control(
        &interrupted,
        &copy,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
        4,
        scope,
        source,
        &NativeRecoveryContinuation {
            applications: Arc::from([]),
            terminal: NativeRecoveryTerminal::Restored(NativeParameterApplication {
                operation: NativeTransientOperation::Clear(target.target()),
                arrangement_frame: 16,
                performance_frame: 24,
                ..applied
            }),
        },
    )
    .unwrap();
    assert_eq!(receipt.events(), 1);
    assert_eq!(receipt.terminal(), end());
    let copy_bytes = std::fs::read(&copy).unwrap();
    let replacement = files.0.join("valid-native-replacement");
    write(&replacement, &session, scope, end());
    let before = session.project().clone();
    assert!(
        recover_sealed(
            &session,
            None,
            scope.target.draft_target(),
            scope.stamp,
            &replacement,
            AutomationRecoveryFileIdentity::Copy(receipt),
        )
        .is_err()
    );
    assert_eq!(session.project(), &before);
    let (command, publication) = recover_sealed(
        &session,
        None,
        scope.target.draft_target(),
        scope.stamp,
        &copy,
        AutomationRecoveryFileIdentity::Copy(receipt),
    )
    .unwrap();
    assert!(matches!(publication, TakePublication::Native(found) if found == target));
    session.editor.execute(*command).unwrap();
    let after = session.project().clone();
    let lane = after
        .device(target.device())
        .unwrap()
        .automation_target(target.target())
        .unwrap();
    assert_eq!(lane.touch.len(), 1);
    assert_eq!(lane.touch[0].steps[0].value.get(), 0.25);
    assert_eq!(
        lane.touch[0].end,
        after.tempo_map().frame_to_beat(16).unwrap()
    );
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &after);
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&after).unwrap()).unwrap(),
        after
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(std::fs::read(copy).unwrap(), copy_bytes);
}

#[test]
fn selecting_a_finished_file_is_read_only_and_cannot_adopt_replacement_bytes() {
    let (files, mut session, scope, _) = fixture();
    let path = files.0.join("selected");
    write(&path, &session, scope, end());
    let before = session.project().clone();
    let selected = inspect_sealed_file(&path).unwrap();
    assert_eq!(selected.path, path);
    assert_eq!(selected.target, target(scope));
    assert_eq!(selected.provenance, scope.stamp);
    assert_eq!(session.project(), &before);
    let original = std::fs::read(&path).unwrap();
    let replacement = files.0.join("another-finished");
    write(
        &replacement,
        &session,
        scope,
        DraftTerminal::Finished(DraftClock {
            performance_frame: 25,
            content_frame: 17,
            loop_pass: 0,
        }),
    );
    let identity = AutomationRecoveryFileIdentity::Sealed(selected.identity);
    std::fs::write(&path, std::fs::read(&replacement).unwrap()).unwrap();
    assert!(
        recover_sealed(
            &session,
            None,
            selected.target,
            selected.provenance,
            &path,
            identity
        )
        .is_err()
    );
    assert_eq!(session.project(), &before);
    std::fs::write(&path, &original).unwrap();
    let (command, _) = recover_sealed(
        &session,
        None,
        selected.target,
        selected.provenance,
        &path,
        identity,
    )
    .unwrap();
    session.editor.execute(*command).unwrap();
    assert_ne!(session.project(), &before);
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let unfinished = files.0.join("unfinished");
    write(&unfinished, &session, scope, DraftTerminal::Interrupted);
    assert!(inspect_sealed_file(&unfinished).is_err());
}
