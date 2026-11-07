//! Actual Root admission and late close around original worker result delivery.
use super::super::tests::{dispose, root, source_reservation, start};
use super::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_parameter_hold_survives_close_during_admission() {
    parameter_close_admission(true);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_parameter_point_survives_close_during_admission() {
    parameter_close_admission(false);
}

fn parameter_close_admission(hold: bool) {
    if std::env::var_os("HEX_HOLD_CLOSE_ADMISSION_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_HOLD_CLOSE_ADMISSION_DRIVER", "1")
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if hold { "144" } else { "92" },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let token = loop {
        let result = if hold {
            root.parameter_hold_admission(
                binding,
                NativeParameterHold::Begin {
                    id: 7,
                    normalized: 0.75,
                },
                || root.close(),
            )
        } else {
            root.parameter_change_admission(binding, 0, 0.75, false, || root.close())
        };
        match result {
            Ok(token) => break token,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("admitted parameter token was lost: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    let (source, operation, parameter) = token.delivery_identity().unwrap();
    assert_eq!(source, binding);
    assert_ne!(operation, 0);
    assert_eq!(parameter, if hold { 7 } else { 0 });
    assert!(!token.original_worker_released_on_control());
    assert!(
        root.begin_native_parameter_change(binding, parameter, 0.25, false)
            .is_err()
    );
    assert!(root.poll_native_parameter_change(binding, &token).is_err());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(token.original_worker_released_on_control());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_controller_query_survives_close_during_admission() {
    use hex_plugin_host::protocol::ParameterPageQuery;
    if std::env::var_os("HEX_CLOSE_ADMISSION_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_CLOSE_ADMISSION_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root
            .controller_query_admission(binding, ParameterPageQuery::default(), || root.close())
        {
            Ok(request) => break request,
            Err("native preparation is busy" | "native controller mailbox is busy") => {}
            Err(reason) => panic!("admitted controller token was lost: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(!request.original_worker_released_on_control());
    assert!(
        root.poll_native_controller_query(binding, &request)
            .is_err()
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(
        matches!(
            returned.error,
            None | Some(hex_plugin_host::Error::Cancelled)
        ),
        "unexpected retirement error: {:?}",
        returned.error
    );
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(request.original_worker_released_on_control());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_controller_query_requires_original_worker_and_suppresses_late_close() {
    use crate::plugin_worker::NativeWorkerControllerProgress;
    use hex_plugin_host::protocol::ParameterPageQuery;
    if std::env::var_os("HEX_ROOT_CONTROLLER_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ROOT_CONTROLLER_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    root.start_native_effect(binding).unwrap();
    assert!(
        root.begin_native_controller_query(
            Binding {
                epoch: binding.epoch + 1,
                ..binding
            },
            ParameterPageQuery::default()
        )
        .is_err()
    );
    {
        let _work = root.work.try_lock().unwrap();
        assert!(matches!(
            root.begin_native_controller_query(binding, ParameterPageQuery::default()),
            Err("native preparation is busy")
        ));
    }
    let request = root
        .begin_native_controller_query(binding, ParameterPageQuery::default())
        .unwrap();
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = root.with_live_capture(binding, |owner| {
            let Owner::Effect(owner) = owner else {
                panic!("original unsplit worker");
            };
            let result = owner.poll_controller_query(&request)?;
            if let NativeWorkerControllerProgress::Page(page) = &result {
                assert_eq!(page.as_slice()[0].id, 7);
                assert_eq!(page.as_slice()[0].normalized, 0.25);
                root.close();
            }
            Ok(result)
        });
        match result {
            Err("native capture source is closing") => break,
            Ok(NativeWorkerControllerProgress::Pending) => {}
            _ => panic!("close must suppress actual controller data"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(
        root.poll_native_controller_query(binding, &request)
            .is_err()
    );
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_live_capture_refuses_pending_stale_and_close_after_result() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    assert!(root.begin_live_native_capture(binding).is_err());
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    assert!(
        root.begin_live_native_capture(binding).is_err(),
        "Prepared has no joining worker"
    );
    root.start_native_effect(binding).unwrap();
    assert!(
        root.begin_live_native_capture(Binding {
            epoch: binding.epoch + 1,
            ..binding
        })
        .is_err()
    );
    {
        let _work = root.work.try_lock().unwrap();
        assert!(matches!(
            root.begin_live_native_capture(binding),
            Err("native preparation is busy")
        ));
    }
    let request = root.begin_live_native_capture(binding).unwrap();
    assert!(root.begin_live_native_capture(binding).is_err());
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let result = root.with_live_capture(binding, |owner| {
            let Owner::Effect(owner) = owner else {
                panic!("original unsplit effect");
            };
            let result = owner.poll_state_capture(&request)?;
            if matches!(result, NativeWorkerCaptureProgress::Captured(_)) {
                root.close();
            }
            Ok(result)
        });
        match result {
            Err("native capture source is closing") => break,
            Ok(NativeWorkerCaptureProgress::Pending) => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            _ => panic!("close must suppress the actual completed result"),
        }
    }
    assert!(root.poll_live_native_capture(binding, &request).is_err());
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_live_capture_sdk_refusal_retains_confirmed_state_and_disposal() {
    if std::env::var_os("HEX_ROOT_CAPTURE_REFUSAL_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env("HEX_ROOT_CAPTURE_REFUSAL_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "69")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let confirmed = root.native_source_prepared_state(binding).unwrap();
    let request = root.begin_live_native_capture(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match root.poll_live_native_capture(binding, &request) {
            Ok(NativeWorkerCaptureProgress::Failed) => break,
            Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            _ => panic!("SDK refusal must not provide captured state"),
        }
    }
    assert_eq!(root.native_source_prepared_state(binding), Some(confirmed));
    drop(request);
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(
        returned.error,
        Some(hex_plugin_host::Error::Worker(6))
    ));
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if !retired.status.success() && retired.terminal.is_none())
    );
}
