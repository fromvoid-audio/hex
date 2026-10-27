//! Actual Root admission and late close around original worker result delivery.
use super::super::tests::{dispose, root, source_reservation, start};
use super::*;
use std::time::{Duration, Instant};

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
