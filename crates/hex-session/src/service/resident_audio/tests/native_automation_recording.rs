//! Actual Session request routing with a controlled applied output witness.
use super::*;
use crate::service::automation_draft::{DraftProvenance, discover_on_control};
use hex_engine::{NativeParameterApplication, NativeTransientOperation, NativeTransientValue};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, HostGain};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
use hex_storage::automation_draft::{DraftAbortReason, DraftOutcome, DraftReadLimits, DraftReader};
use std::sync::atomic::AtomicU64;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-native-take-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn successive_native_takes_keep_the_arm_and_refusals_keep_the_recovery_file() {
    use DeviceAutomationTakeAction as Action;
    use DeviceAutomationTakeProgress as Progress;
    let scratch = Scratch::new();
    let observed = Arc::new(Observed::default());
    let runtime = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        1024,
        Some(scratch.0.join("recovery.hex-journal")),
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: 2 * runtime,
                max_construction_peak_bytes: 3 * runtime,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    receive();
    let submit = |request| {
        service.submit(request).unwrap();
        receive()
    };
    let added = submit(Request::Edit(Command::InsertMainDevice {
        before: None,
        state: hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
            .unwrap(),
    }));
    let Some(Created::Device(device)) = added.created else {
        panic!("main effect: {:?}", added.error);
    };
    let target = DeviceAutomationTarget::HostOutputGain;
    let authored = submit(Request::Edit(Command::ReplaceDeviceAutomationTarget {
        device,
        target,
        points: vec![AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(0.5).unwrap(),
            outgoing: AutomationInterpolation::Step,
        }],
    }));
    assert!(authored.error.is_none(), "{:?}", authored.error);
    let revision = authored.state.revision;
    let mut queried = submit(Request::QueryFailedPluginAutomation { request_id: 912 });
    assert!(queried.error.is_none());
    let completion = queried.device_automation_take_complete.take().unwrap();
    assert_eq!(completion.request_id, 912);
    assert_eq!(completion.progress, Progress::FailedPluginUnavailable);
    assert_eq!(queried.state.revision, revision);
    let scope = || {
        let armed = submit(Request::ArmNativeParameterRecording {
            project_generation: 0,
            revision,
            target: Some((device, target)),
        });
        assert!(armed.error.is_none(), "{:?}", armed.error);
        let audio_revision = observed.publications.lock().unwrap().len() as u64;
        DeviceAutomationTakeScope {
            target: hex_engine::native_parameter_input_target(&armed.state.project, device, target)
                .unwrap()
                .into(),
            stamp: DraftProvenance {
                sample_rate: 48_000,
                project_generation: 0,
                editor_revision: revision,
                output_identity: 7,
                audio_revision,
                transport_epoch: 3,
                gesture_serial: audio_revision,
            },
        }
    };
    let next = AtomicU64::new(1);
    let send = |scope, action| {
        let request_id = next.fetch_add(1, Ordering::Relaxed);
        let mut event = submit(Request::DeviceAutomationTake {
            request_id,
            scope,
            action,
        });
        let complete = event.device_automation_take_complete.take().unwrap();
        assert_eq!(complete.request_id, request_id);
        (event, complete.progress)
    };
    let wait = |scope| {
        let until = Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            let result = send(scope, Action::Poll);
            if result.1 != Progress::Pending {
                break result;
            }
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(1));
        }
    };
    let draft_directory = scratch.0.join("automation-drafts");
    let initial_scope = scope();
    let (no_output, no_output_progress) = send(
        initial_scope,
        Action::AdoptFailedPluginCopy { max_events: 2 },
    );
    assert_eq!(no_output_progress, Progress::Refused);
    assert!(no_output.error.is_some());
    assert_eq!(no_output.state.revision, revision);
    let missing_copy = scratch.0.join("missing-plugin-copy");
    let (missing_event, missing_progress) = send(
        initial_scope,
        Action::CopyFailedPlugin {
            destination: missing_copy.clone(),
            max_events: 2,
        },
    );
    assert_eq!(missing_progress, Progress::Refused);
    assert!(missing_event.error.is_some());
    assert!(!missing_copy.exists());
    assert_eq!(send(initial_scope, Action::Begin).1, Progress::Refused);
    assert!(!draft_directory.exists());
    observed
        .native_capture_playing
        .store(true, Ordering::Release);
    let wrong_epoch = DeviceAutomationTakeScope {
        stamp: DraftProvenance {
            transport_epoch: 2,
            ..initial_scope.stamp
        },
        ..initial_scope
    };
    assert_eq!(send(wrong_epoch, Action::Begin).1, Progress::Refused);
    assert!(!draft_directory.exists());

    let application =
        |scope: DeviceAutomationTakeScope, frame, operation| NativeParameterApplication {
            identity: scope.target.native().unwrap().identity(),
            operation,
            arrangement_frame: frame,
            loop_pass: 0,
            performance_frame: frame + 20,
            remaining_latency_frames: 7,
        };
    let record = |scope, frame| {
        assert_eq!(send(scope, Action::Begin).1, Progress::Started);
        let value = NativeTransientValue::HostOutputGain(HostGain::from_normalized(0.25).unwrap());
        let points = Arc::from([(
            1,
            application(scope, frame, NativeTransientOperation::Set(value)),
        )]);
        let wrong_target = DeviceAutomationTakeScope {
            target: hex_engine::native_parameter_input_target(
                &authored.state.project,
                device,
                DeviceAutomationTarget::HostInputGain,
            )
            .unwrap()
            .into(),
            ..scope
        };
        assert_eq!(
            send(wrong_target, Action::Append(Arc::clone(&points))).1,
            Progress::Refused
        );
        assert_eq!(send(scope, Action::Append(points)).1, Progress::Appended);
    };

    record(initial_scope, 8);
    let blocked_disarm = submit(Request::ArmNativeParameterRecording {
        project_generation: 0,
        revision,
        target: None,
    });
    assert!(blocked_disarm.error.is_some());
    assert_eq!(
        *observed.native_parameter_input.lock().unwrap(),
        initial_scope.target.native()
    );
    assert_eq!(
        send(initial_scope, Action::Abort(DraftAbortReason::Cancelled)).1,
        Progress::Sealing
    );
    let (aborted, result) = wait(initial_scope);
    let Progress::Retained(path) = result else {
        panic!("{:?}", aborted.error)
    };
    assert_eq!(aborted.state.project, authored.state.project);
    let limits = DraftReadLimits {
        max_source_bytes: 64 * 1024,
    };
    assert!(matches!(
        DraftReader::open(&path, limits).unwrap().outcome(),
        DraftOutcome::Aborted(_)
    ));

    let refused_scope = scope();
    record(refused_scope, 8);
    let finish = |scope, frame| {
        assert_eq!(
            send(
                scope,
                Action::Finish(application(
                    scope,
                    frame,
                    NativeTransientOperation::Clear(target)
                ))
            )
            .1,
            Progress::Sealing,
        );
    };
    finish(refused_scope, 12);
    observed.refuse.store(true, Ordering::Release);
    let (refused, result) = wait(refused_scope);
    let Progress::Retained(path) = result else {
        panic!("{:?}", refused.error)
    };
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, authored.state.project);
    assert!(automation_take::CompletedAutomationTake::open_on_control(&path, limits).is_ok());

    observed.refuse.store(false, Ordering::Release);
    let final_scope = scope();
    record(final_scope, 8);
    finish(final_scope, 12);
    let (committed, result) = wait(final_scope);
    assert_eq!(result, Progress::Committed, "{:?}", committed.error);
    assert_eq!(committed.state.revision, revision + 1);
    let lane = committed.state.project.main_devices()[0]
        .automation_target(target)
        .unwrap();
    let map = committed.state.project.tempo_map();
    for (frame, expected) in [(7, 0.5), (8, 0.25), (11, 0.25), (12, 0.5)] {
        assert_eq!(
            lane.value_at(map.frame_to_beat(frame).unwrap())
                .unwrap()
                .get(),
            expected
        );
    }
    assert_eq!(send(final_scope, Action::Begin).1, Progress::Refused);
    assert_eq!(
        *observed.native_parameter_input.lock().unwrap(),
        final_scope.target.native()
    );
    let audio_revision = observed.publications.lock().unwrap().len() as u64;
    let second_scope = DeviceAutomationTakeScope {
        stamp: DraftProvenance {
            editor_revision: committed.state.revision,
            audio_revision,
            gesture_serial: final_scope.stamp.gesture_serial + 1,
            ..final_scope.stamp
        },
        ..final_scope
    };
    let unapplied = DeviceAutomationTakeScope {
        stamp: DraftProvenance {
            audio_revision: final_scope.stamp.audio_revision,
            ..second_scope.stamp
        },
        ..second_scope
    };
    assert_eq!(send(unapplied, Action::Begin).1, Progress::Refused);
    record(second_scope, 16);
    finish(second_scope, 20);
    let (second, result) = wait(second_scope);
    assert_eq!(result, Progress::Committed, "{:?}", second.error);
    assert_eq!(
        second.state.project.main_devices()[0]
            .automation_target(target)
            .unwrap()
            .touch
            .len(),
        2
    );
    assert_eq!(submit(Request::Undo).state.project, committed.state.project);
    assert_eq!(*observed.native_parameter_input.lock().unwrap(), None);
    assert_eq!(submit(Request::Undo).state.project, authored.state.project);
    assert_eq!(submit(Request::Redo).state.project, committed.state.project);
    assert_eq!(submit(Request::Redo).state.project, second.state.project);
    assert_eq!(
        discover_on_control(&draft_directory, 8)
            .unwrap()
            .candidates
            .len(),
        4
    );
}
