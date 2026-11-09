use super::*;
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId};
use hex_project::{Command, Created};
use hex_session::{
    Session,
    service::{
        DeviceAutomationSource, NativeRecoveryContinuation, NativeRecoveryTerminal,
        automation_draft::{DeviceDraftInput, DraftProvenance, Vst3CaptureTarget},
        copy_native_recovery_on_control,
    },
};
use hex_storage::automation_draft::{CaptureClock, DraftReadLimits};
use std::sync::Arc;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cancelled_picker_and_stale_responses_cannot_release_the_original() {
    let directory = std::env::temp_dir().join(format!(
        "hex-editor-automation-recovery-{}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let directory = Scratch(directory);
    let original = directory.0.join("original");
    let destination = directory.0.join("copy");
    let mut session = Session::new(48_000, 4, 4096).unwrap();
    let Created::Device(device) = session
        .command(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("fixture did not create a device");
    };
    let target = hex_engine::native_parameter_input_target(
        session.project(),
        device,
        DeviceAutomationTarget::HostOutputGain,
    )
    .unwrap();
    let stamp = DraftProvenance {
        sample_rate: 48_000,
        output_identity: 7,
        project_generation: 0,
        editor_revision: 1,
        audio_revision: 2,
        transport_epoch: 3,
        gesture_serial: 4,
    };
    let scope = Scope {
        target: target.into(),
        stamp,
    };
    let clock = CaptureClock {
        sample_rate: stamp.sample_rate,
        loop_region: None,
    };
    let (_input, worker) =
        DeviceDraftInput::start_on_control(&session, None, original.clone(), target, stamp, clock)
            .unwrap();
    let outcome = worker.join_on_control().unwrap();
    let receipt = copy_native_recovery_on_control(
        &outcome,
        &destination,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
        1,
        scope,
        DeviceAutomationSource {
            target: scope.target,
            sample_rate: stamp.sample_rate,
            output_identity: stamp.output_identity,
            audio_revision: stamp.audio_revision,
            transport_epoch: stamp.transport_epoch,
            section_loop: None,
        },
        &NativeRecoveryContinuation {
            applications: Arc::from([]),
            terminal: NativeRecoveryTerminal::Interrupted,
        },
    )
    .unwrap();

    let mut flow = Recovery {
        phase: Phase::Choosing { scope, picker: 5 },
        request: None,
        saved: None,
    };
    flow.finish_picker(scope, 4, Some(destination.clone()));
    assert!(flow.matches_picker(scope, 5));
    flow.finish_picker(scope, 5, None);
    assert!(matches!(flow.phase, Phase::NeedsCopy(selected) if selected == scope));
    assert!(flow.unresolved());

    flow.phase = Phase::Choosing { scope, picker: 6 };
    flow.finish_picker(scope, 6, Some(destination.clone()));
    flow.request = Some(7);
    assert_eq!(flow.settle(7, &Progress::Retained(original.clone())), None);
    flow.request = Some(8);
    assert_eq!(flow.settle(8, &Progress::Pending), None);
    let verified = Progress::VerifiedCopy {
        path: destination.clone(),
        receipt,
    };
    flow.request = Some(9);
    assert_eq!(flow.settle(8, &verified), None);
    assert!(flow.matches_request(9));
    assert_eq!(flow.settle(9, &verified), None);
    let Phase::Verified { scope, copy } = std::mem::take(&mut flow.phase) else {
        panic!("matching verified copy was not offered for keep");
    };
    flow.phase = Phase::Keeping { scope, copy };
    flow.request = Some(10);
    assert_eq!(flow.settle(10, &Progress::CopyKept(original.clone())), None);
    assert!(flow.unresolved(), "another path cannot release custody");

    flow.phase = Phase::Keeping {
        scope,
        copy: Verified {
            path: destination.clone(),
            receipt,
        },
    };
    flow.request = Some(11);
    let kept = Progress::CopyKept(destination.clone());
    assert_eq!(flow.settle(10, &kept), None);
    assert!(flow.unresolved());
    assert_eq!(
        flow.settle(11, &kept),
        Some(Resolution::Kept(Box::new(scope)))
    );
    assert!(flow.saved.is_none(), "interrupted copies cannot be applied");
    assert_eq!(flow.settle(11, &kept), None);
    assert!(!flow.unresolved());
    assert!(original.is_file() && destination.is_file());

    use hex_engine::{NativeParameterApplication, NativeTransientOperation, NativeTransientValue};
    let applied = NativeParameterApplication {
        identity: target.identity(),
        operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
            hex_lattice_contract::HostGain::from_normalized(0.25).unwrap(),
        )),
        arrangement_frame: 4,
        performance_frame: 8,
        loop_pass: 0,
        remaining_latency_frames: 4,
    };
    let path = directory.0.join("finished");
    let receipt = copy_native_recovery_on_control(
        &outcome,
        &path,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
        1,
        scope,
        DeviceAutomationSource {
            target: scope.target,
            sample_rate: stamp.sample_rate,
            output_identity: stamp.output_identity,
            audio_revision: stamp.audio_revision,
            transport_epoch: stamp.transport_epoch,
            section_loop: None,
        },
        &NativeRecoveryContinuation {
            applications: Arc::from([(1, applied)]),
            terminal: NativeRecoveryTerminal::Restored(NativeParameterApplication {
                operation: NativeTransientOperation::Clear(target.target()),
                arrangement_frame: 12,
                performance_frame: 16,
                ..applied
            }),
        },
    )
    .unwrap();
    flow.phase = Phase::Keeping {
        scope,
        copy: Verified {
            path: path.clone(),
            receipt,
        },
    };
    flow.request = Some(12);
    assert_eq!(
        flow.settle(12, &Progress::CopyKept(path.clone())),
        Some(Resolution::Kept(Box::new(scope)))
    );
    assert!(
        !flow.unresolved(),
        "a kept file no longer owns close custody"
    );
    let (saved_scope, copy) = flow
        .saved
        .take()
        .expect("finished copy offered for explicit apply");
    assert_eq!(saved_scope, scope);
    assert_eq!(copy.path, path);
    assert_eq!(copy.receipt, receipt);
    flow.phase = Phase::Applying { scope, copy };
    flow.request = Some(13);
    assert_eq!(flow.settle(12, &Progress::Committed), None);
    assert!(
        flow.unresolved(),
        "stale success cannot retire the pending edit"
    );
    assert_eq!(flow.settle(13, &Progress::Refused), None);
    let (_, copy) = flow.saved.take().expect("refused apply remains retryable");
    flow.phase = Phase::Applying { scope, copy };
    flow.request = Some(14);
    assert_eq!(
        flow.settle(14, &Progress::Committed),
        Some(Resolution::Applied)
    );
    assert!(flow.saved.is_none());
    assert!(!flow.unresolved());
}

#[test]
fn only_the_ordered_plugin_query_selects_the_original_scope() {
    let scope = plugin_scope();
    let mut flow = Recovery {
        phase: Phase::Discover {
            until: Instant::now() + Duration::from_secs(1),
        },
        request: Some(1),
        saved: None,
    };
    let found = Progress::FailedPluginCustody {
        scope: Box::new(scope),
        path: "original".into(),
    };
    assert_eq!(flow.settle(0, &found), None);
    assert!(!flow.unresolved());
    assert_eq!(flow.settle(1, &found), None);
    assert!(flow.unresolved());
    assert!(matches!(flow.phase, Phase::NeedsCopy(selected) if selected == scope));
}

fn plugin_scope() -> Scope {
    let mut session = Session::new(48_000, 4, 4096).unwrap();
    let Created::Device(device) = session
        .command(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("fixture did not create a device");
    };
    Scope {
        target: hex_session::service::DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
            device,
            class: [9; 16],
            parameter: 0,
            binding: hex_lattice_contract::ForeignProcessorBinding {
                root: 7,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
        }),
        stamp: DraftProvenance {
            sample_rate: 48_000,
            output_identity: 7,
            project_generation: 0,
            editor_revision: 1,
            audio_revision: 2,
            transport_epoch: 3,
            gesture_serial: 4,
        },
    }
}

#[cfg(all(
    feature = "ui-test-support",
    target_os = "macos",
    target_arch = "aarch64"
))]
#[gpui::test]
fn rendered_copy_action_can_cancel_without_releasing_the_close_fence(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{AnyWindowHandle, AppContext, Empty, Modifiers, VisualTestContext};
    use hex_session::service::Service;

    let service = Service::new(48_000, 4, 4096).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let initial = loop {
        if let Ok(event) = service.try_event() {
            break event;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    cx.use_native_text_system().expect("native text shaping");
    cx.update(|cx| crate::theme::load_fonts(cx).expect("bundled ioskeley mono"));
    let window: AnyWindowHandle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(window, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline fixture".into()))
            });
            editor.update(cx, |view, cx| {
                view.receive(initial);
                view.automation_take_recovery.phase = Phase::NeedsCopy(plugin_scope());
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let copy = visual
        .debug_bounds("automation-take-recovery")
        .expect("recovery action");
    visual.simulate_click(copy.center(), Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    while !visual.did_prompt_for_new_path() {
        visual.run_until_parked();
        assert!(
            Instant::now() < deadline,
            "recovery picker was not requested"
        );
        std::thread::yield_now();
    }
    visual.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert_eq!(
                view.automation_recovery_status(),
                Some(("save automation recovery copy", true))
            );
            assert!(view.automation_take_recovery.unresolved());
            view.request_window_close(window, cx);
            assert!(view.error.as_ref().unwrap().contains("recover"));
        });
        window.remove_window();
    })
    .unwrap();
}
