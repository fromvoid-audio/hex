//! Original Root cell/endpoint/lease renewal; full output controls live at prepared.
use super::*;

fn presentation() -> Option<hex_plugin_host::protocol::PresentationLatency> {
    Some(hex_plugin_host::protocol::PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    })
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_epoch_requires_original_endpoint_return_and_matching_resume_ack() {
    let root = root();
    root.configure_native_sources(2, source_reservation())
        .unwrap();
    let (build, source, preparation) = input();
    let other = root
        .begin_native_source(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 3).unwrap(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    root.complete_native_source(other, presentation()).unwrap();
    root.start_native_effect(other).unwrap();
    let other_state = root.native_source_prepared_state(other).unwrap();
    let mut binding = start(&root);
    root.complete_native_source(binding, presentation())
        .unwrap();
    let child = {
        let work = lock(&root.work);
        let Some(Owner::Prepared(owner)) = work.sources.cells[1].owner.as_ref() else {
            panic!("actual prepared child")
        };
        owner.child_id_on_control()
    };
    let state = root.native_source_prepared_state(binding).unwrap();
    root.start_native_effect(binding).unwrap();
    for _ in 0..3 {
        let endpoint = root.loan_native_endpoint(binding).unwrap();
        assert!(
            root.quiesce_native_source(binding, binding.graph + 1)
                .is_err()
        );
        assert_eq!(lock(&root.bank).sources.slots[1].binding, Some(binding));
        assert!(root.ingest_native_endpoint(endpoint).is_ok());
        assert!(root.quiesce_native_source(binding, 0).is_err());
        assert!(
            root.quiesce_native_source(binding, binding.graph - 1)
                .is_err()
        );
        let next = root
            .quiesce_native_source(binding, binding.graph + 1)
            .unwrap();
        assert_eq!((next.root, next.instance), (binding.root, binding.instance));
        assert!(next.epoch > binding.epoch);
        assert!(root.native_source_progress(binding).is_none());
        assert!(!root.retire_native_source(binding));
        assert!(matches!(
            root.native_source_progress(next),
            Some(NativeSourcePreparationProgress::Facts(_))
        ));
        assert!(root.native_source_prepared_state(next).is_none());
        assert!(root.capture_native_source(binding, &mut || false).is_err());
        {
            let _work = lock(&root.work);
            assert_eq!(
                root.capture_native_source(next, &mut || false),
                Err("native preparation is busy")
            );
        }
        let captured = root
            .capture_native_source(next, &mut || false)
            .unwrap()
            .unwrap();
        assert_eq!(captured, state);
        assert_eq!(
            root.capture_native_source(next, &mut || false).unwrap(),
            Some(captured)
        );
        assert!(
            root.native_source_prepared_state(next).is_none(),
            "capture data is not a resume ACK"
        );
        assert!(root.loan_native_endpoint(next).is_err());
        assert!(root.start_native_effect(next).is_err());
        assert!(
            root.with_native_effect_definition(&[next], |_| Ok(()))
                .is_err()
        );
        {
            let work = lock(&root.work);
            let Some(Owner::Quiescent(owner)) = work.sources.cells[1].owner.as_ref() else {
                panic!("actual stopped source")
            };
            assert_eq!(
                owner.child_id_on_control(),
                child,
                "diagnostic alongside retained original owner"
            );
            assert_eq!(owner.completed_blocks(), 0);
        }
        assert_eq!(
            root.native_source_prepared_state(other).as_ref(),
            Some(&other_state)
        );
        assert!(
            root.with_native_effect_definition(&[other], |_| Ok(()))
                .is_ok()
        );
        let unrelated_endpoint = root.loan_native_endpoint(other).unwrap();
        assert!(root.ingest_native_endpoint(unrelated_endpoint).is_ok());
        root.complete_native_source(next, presentation()).unwrap();
        assert_eq!(root.native_source_prepared_state(next), Some(state.clone()));
        root.start_native_effect(next).unwrap();
        assert!(matches!(
            root.native_source_progress(next),
            Some(NativeSourcePreparationProgress::Effect { .. })
        ));
        binding = next;
    }
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success() && retired.terminal.is_some())
    );
    assert!(root.retire_native_source(other));
    dispose(&root, other);
    let returned = root.recover_native_source(other).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|t| t.blocks == 0 && t.frames == 0))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_capture_cancel_retains_original_failure_until_disposal() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let old = start(&root);
    assert!(
        root.capture_native_source(old, &mut || false).is_err(),
        "pending is not quiescent"
    );
    root.complete_native_source(old, presentation()).unwrap();
    root.start_native_effect(old).unwrap();
    let confirmed = root.native_source_prepared_state(old).unwrap();
    assert!(
        root.capture_native_source(old, &mut || false).is_err(),
        "live is not quiescent"
    );
    let endpoint = root.loan_native_endpoint(old).unwrap();
    assert!(
        root.capture_native_source(old, &mut || false).is_err(),
        "loan has not returned"
    );
    assert!(root.ingest_native_endpoint(endpoint).is_ok());
    let next = root.quiesce_native_source(old, old.graph + 1).unwrap();
    assert_eq!(
        root.capture_native_source(next, &mut || true).unwrap(),
        None
    );
    assert!(matches!(
        root.native_source_progress(next),
        Some(NativeSourcePreparationProgress::Retiring)
    ));
    assert!(
        root.recover_native_source(next).is_none(),
        "refusal is not disposal"
    );
    dispose(&root, next);
    let returned = root.recover_native_source(next).unwrap();
    assert!(matches!(
        returned.error,
        Some(hex_plugin_host::Error::Cancelled)
    ));
    assert!(
        matches!(returned.disposal,PreparationDisposal::Reaped(retired) if retired.status.code()==Some(9))
    );
    assert!(
        hex_plugin_state::decode(&confirmed)
            .unwrap()
            .component
            .is_some()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_close_during_capture_retires_without_publishing_captured_bytes() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let old = start(&root);
    root.complete_native_source(old, presentation()).unwrap();
    root.start_native_effect(old).unwrap();
    let confirmed = root.native_source_prepared_state(old).unwrap();
    let next = root.quiesce_native_source(old, old.graph + 1).unwrap();
    let mut closed = false;
    let result = root.capture_native_source(next, &mut || {
        if !closed {
            closed = true;
            root.close();
        }
        false
    });
    assert!(closed);
    assert_eq!(result, Ok(None));
    assert!(root.native_source_prepared_state(next).is_none());
    assert!(root.complete_native_source(next, presentation()).is_err());
    dispose(&root, next);
    let returned = root.recover_native_source(next).unwrap();
    assert!(matches!(
        returned.error,
        Some(hex_plugin_host::Error::Cancelled)
    ));
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.code() == Some(9))
    );
    assert!(
        hex_plugin_state::decode(&confirmed)
            .unwrap()
            .component
            .is_some()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_epoch_completion_refusal_and_late_close_retain_disposal() {
    for mode in 0..3 {
        let root = root();
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let old = start(&root);
        root.complete_native_source(old, presentation()).unwrap();
        root.start_native_effect(old).unwrap();
        let next = root
            .quiesce_native_source_between(old, old.graph + 1, || {
                if mode == 1 {
                    root.close();
                }
            })
            .unwrap();
        if mode == 0 {
            // The actual native facts require presentation; refusal is retained,
            // never an accepted resumed effect or an implicit retry.
            root.complete_native_source(next, None).unwrap();
        }
        if mode == 2 {
            assert!(root.retire_native_source(next));
            assert!(root.complete_native_source(next, presentation()).is_err());
        }
        assert!(root.start_native_effect(next).is_err());
        dispose(&root, next);
        let returned = root.recover_native_source(next).unwrap();
        if mode != 2 {
            assert!(returned.error.is_some());
        }
        assert!(
            matches!(returned.disposal, PreparationDisposal::Reaped(retired) if !retired.status.success() && retired.terminal.is_none())
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_stopped_parameters_require_original_return_and_keep_failure_or_resume_custody() {
    if std::env::var_os("HEX_ROOT_PARAMETER_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        for (mode, variant) in [("apply", "37"), ("refuse", "76"), ("cancel", "37")] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &name,
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env("HEX_ROOT_PARAMETER_DRIVER", mode)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        }
        return;
    }
    let mode = std::env::var("HEX_ROOT_PARAMETER_DRIVER").unwrap();
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(binding, presentation())
        .unwrap();
    let confirmed = root.native_source_prepared_state(binding).unwrap();
    let child = {
        let work = lock(&root.work);
        let Some(Owner::Prepared(owner)) = work.sources.cells[0].owner.as_ref() else {
            panic!("original prepared source")
        };
        owner.child_id_on_control()
    };
    root.start_native_effect(binding).unwrap();
    let mut points = hex_plugin_host::transport::Parameters::default();
    points
        .push(hex_plugin_host::transport::ParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.75,
        })
        .unwrap();
    assert!(
        root.flush_native_parameters(binding, &points, &mut || false)
            .is_err(),
        "active source cannot flush"
    );
    let endpoint = root.loan_native_endpoint(binding).unwrap();
    assert!(
        root.quiesce_native_source(binding, binding.graph + 1)
            .is_err()
    );
    assert!(root.ingest_native_endpoint(endpoint).is_ok());
    let next = root
        .quiesce_native_source(binding, binding.graph + 1)
        .unwrap();
    assert!(
        root.flush_native_parameters(binding, &points, &mut || false)
            .is_err(),
        "old reservation is stale"
    );
    {
        let _work = lock(&root.work);
        assert!(matches!(
            root.flush_native_parameters(next, &points, &mut || false),
            Err("native preparation is busy")
        ));
    }
    let result = root
        .flush_native_parameters(next, &points, &mut || mode == "cancel")
        .unwrap();
    if mode != "apply" {
        assert!(
            result.is_none(),
            "failure cannot publish a readback or resume"
        );
        assert!(root.complete_native_source(next, presentation()).is_err());
        dispose(&root, next);
        let returned = root.recover_native_source(next).unwrap();
        if mode == "refuse" {
            assert!(matches!(
                returned.error,
                Some(hex_plugin_host::Error::Worker(11))
            ))
        } else {
            assert!(matches!(
                returned.error,
                Some(hex_plugin_host::Error::Cancelled)
            ))
        }
        assert_eq!(
            &hex_plugin_state::decode(&confirmed)
                .unwrap()
                .component
                .unwrap()[8..12],
            &1.0_f32.to_le_bytes()
        );
        return;
    }
    let values = result.unwrap();
    assert_eq!(
        values.as_slice(),
        &[hex_plugin_host::protocol::FlushedParameter {
            id: 7,
            normalized: 0.75
        }]
    );
    assert!(
        root.native_source_prepared_state(next).is_none(),
        "flush is no resume ACK"
    );
    assert!(root.loan_native_endpoint(next).is_err());
    let captured = root
        .capture_native_source(next, &mut || false)
        .unwrap()
        .unwrap();
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    {
        let work = lock(&root.work);
        let Some(Owner::Quiescent(owner)) = work.sources.cells[0].owner.as_ref() else {
            panic!("same stopped source")
        };
        assert_eq!(owner.child_id_on_control(), child);
        assert_eq!(owner.completed_blocks(), 0);
    }
    root.complete_native_source(next, presentation()).unwrap();
    assert!(root.native_source_prepared_state(next).is_some());
    root.start_native_effect(next).unwrap();
    // The fresh worker can capture the original stopped flush without any
    // invented PCM process call. This gives stopped controls the same actual
    // capture/permission path as playing controls after the real resume ACK.
    let request = root.begin_live_native_capture(next).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        match root.poll_live_native_snapshot(next, &request).unwrap() {
            crate::plugin_worker::NativeWorkerStateSnapshotProgress::Captured(snapshot) => {
                break snapshot;
            }
            crate::plugin_worker::NativeWorkerStateSnapshotProgress::Pending => {}
            other => panic!("resumed stopped-parameter snapshot: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert_eq!(
        &hex_plugin_state::decode(snapshot.state())
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert!(
        !root
            .matches_native_accepted_state(next, snapshot.state())
            .unwrap()
    );
    root.stage_live_native_snapshot(next, &snapshot).unwrap();
    assert_eq!(
        root.matches_native_accepted_state(next, snapshot.state()),
        Err("native state approval is pending")
    );
    root.finalize_live_native_snapshot(next, &snapshot, true)
        .unwrap();
    assert!(
        root.matches_native_accepted_state(next, snapshot.state())
            .unwrap()
    );
    assert!(
        !root
            .matches_native_accepted_state(next, &confirmed)
            .unwrap()
    );
    assert!(root.retire_native_source(next));
    dispose(&root, next);
    let returned = root.recover_native_source(next).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal,PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some_and(|terminal| terminal.blocks == 0))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_quiescence_preserves_pending_state_terminal_before_rebinding() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(binding, presentation())
        .unwrap();
    root.start_native_effect(binding).unwrap();
    let request = root.begin_live_native_capture(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        match root.poll_live_native_snapshot(binding, &request).unwrap() {
            crate::plugin_worker::NativeWorkerStateSnapshotProgress::Captured(snapshot) => {
                break snapshot;
            }
            crate::plugin_worker::NativeWorkerStateSnapshotProgress::Pending => {}
            other => panic!("original state snapshot: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    root.stage_live_native_snapshot(binding, &snapshot).unwrap();
    assert_eq!(
        root.quiesce_native_source(binding, binding.graph),
        Err("native epoch requires state approval terminal")
    );
    assert_eq!(lock(&root.bank).sources.slots[0].binding, Some(binding));
    root.finalize_live_native_snapshot(binding, &snapshot, false)
        .unwrap();
    root.stage_live_native_snapshot(binding, &snapshot).unwrap();
    root.finalize_live_native_snapshot(binding, &snapshot, true)
        .unwrap();
    let next = root.quiesce_native_source(binding, binding.graph).unwrap();
    assert!(next.epoch > binding.epoch);
    assert!(lock(&root.work).sources.cells[0].suspended_state.is_some());
    assert!(root.retire_native_source(next));
    dispose(&root, next);
    assert!(root.recover_native_source(next).unwrap().error.is_none());
    assert!(
        lock(&root.work).sources.cells[0].suspended_state.is_none(),
        "recovered pool cells cannot lend accepted data to a replacement source"
    );
}
