//! Public live capture through actual ordinary graph and installed endpoint.
use super::*;
use crate::plugin_worker::{NativeWorkerCapture, NativeWorkerCaptureProgress};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_input_terminal_keeps_original_root_binding_until_drained() {
    use crate::plugin_worker::NativeWorkerParameterProgress;
    let _serial = serial();
    for location in 0..3 {
        let mut f = fixture_at("processing", location);
        let canonical = f.editor.project().clone();
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 1);
        let until = Instant::now() + Duration::from_secs(3);
        let request = loop {
            match f
                .control
                .begin_native_source_parameter_change_on_control(f.binding, 7, 0.75)
            {
                Ok(request) => break request,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(error) => panic!("input admission: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        let source = f.control.status.native_effect_source().unwrap();
        f.control.stop();
        let fence = f.control.request_native_epoch_fence(source).unwrap();
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        assert_eq!(
            f.control.stopped_edit_fence_state(fence),
            StoppedEditFenceState::Ready
        );
        f.control
            .return_stopped_ordinary_native_epoch_runtime(source, fence)
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            f.control.reclaim();
            if f.control.native_source_endpoint_status(f.binding) == Some((false, false)) {
                break;
            }
            assert!(Instant::now() < until, "original endpoint did not return");
            allocation_free(|| {
                f.data
                    .owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
        }
        let old = f.binding;
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f.control.quiesce_native_source_preparation(old, old.graph) {
                Err("native epoch requires parameter input terminal") => break,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                other => panic!("pending input must block rebinding: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(
            f.control
                .drain_stopped_native_source_parameter_change_on_control(
                    Binding {
                        epoch: old.epoch + 1,
                        ..old
                    },
                    &request
                )
                .is_err()
        );
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .drain_stopped_native_source_parameter_change_on_control(old, &request)
            {
                Ok(NativeWorkerParameterProgress::Withdrawn) => break,
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
                other => panic!("queued input must be withdrawn after actual stop: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(
            f.control
                .drain_stopped_native_source_parameter_change_on_control(old, &request)
                .is_err()
        );
        assert_eq!(f.editor.project(), &canonical);
        renew_cohort(&mut f);
        assert!(f.binding.epoch > old.epoch);
        assert!(
            f.control
                .drain_stopped_native_source_parameter_change_on_control(f.binding, &request)
                .is_err()
        );
        f.control.release_stopped_edit_fence(fence);
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some())
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_track_return_main_host_input_changes_pcm_without_claiming_canonical_commit() {
    use crate::plugin_worker::NativeWorkerParameterProgress;
    if std::env::var_os("HEX_ORDINARY_HOST_INPUT_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ORDINARY_HOST_INPUT_DRIVER", "1")
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
    let _serial = serial();
    for location in 0..3 {
        let mut f = fixture_at("processing", location);
        let canonical = f.editor.project().clone();
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 1);
        let until = Instant::now() + Duration::from_secs(3);
        let request = loop {
            match f
                .control
                .begin_native_source_parameter_change_on_control(f.binding, 7, 0.75)
            {
                Ok(request) => break request,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(error) => panic!("original source host-input admission: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        assert!(
            f.control
                .poll_native_source_parameter_change_on_control(
                    Binding {
                        epoch: f.binding.epoch + 1,
                        ..f.binding
                    },
                    &request
                )
                .is_err()
        );
        for n in 1..5_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, n + 1);
            let input_gain = if n < 3 { 1.0 } else { 3.0 };
            let gain = input_gain * (0.5 + n.saturating_sub(2) as f32 * 0.125)
                + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.25 * gain, -0.5 * gain]; 8]
                }
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        let processed = loop {
            match f
                .control
                .poll_native_source_parameter_change_on_control(f.binding, &request)
            {
                Ok(NativeWorkerParameterProgress::Processed(value)) => break value,
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
                other => panic!("original source host-input terminal: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        assert_eq!(
            (
                processed.point.id,
                processed.point.normalized,
                processed.sequence,
                processed.first_frame
            ),
            (7, 0.75, 1, 8)
        );
        let capture = begin(&mut f);
        let actual = captured(&mut f, &capture);
        assert_eq!(
            &hex_plugin_state::decode(&actual)
                .unwrap()
                .component
                .unwrap()[8..12],
            &3.0_f32.to_le_bytes()
        );
        assert_eq!(
            f.editor.project(),
            &canonical,
            "input/capture is not canonical commit authority"
        );
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(initial)
        );
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        dispose(f);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_hold_requires_the_matching_applied_output_and_restores_read() {
    use crate::plugin_worker::{
        NativeParameterHold as Hold, NativeWorkerParameterProgress as Progress,
    };
    if std::env::var_os("HEX_ORDINARY_HOLD_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ORDINARY_HOLD_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }
    let _serial = serial();
    let mut f = fixture_with_authored("processing", 2, 8, &[7]);
    let canonical = f.editor.project().clone();
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let source = AppliedAudioSource {
        context: context(),
        audio_revision: revision,
        output_identity: f.control.output_identity().unwrap(),
    };
    let begin = Hold::Begin {
        id: 7,
        normalized: 0.75,
    };
    assert!(
        f.control
            .native_source_parameter_hold_on_control(source, f.binding, begin)
            .is_err()
    );
    f.control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, f.binding, 1);
    assert_eq!(f.control.status.native_effect_source(), Some(source));
    let stale = AppliedAudioSource {
        audio_revision: revision + 1,
        ..source
    };
    assert!(
        f.control
            .native_source_parameter_hold_on_control(stale, f.binding, begin)
            .is_err()
    );
    let foreign = Binding {
        epoch: f.binding.epoch + 1,
        ..f.binding
    };
    assert!(
        f.control
            .native_source_parameter_hold_on_control(source, foreign, begin)
            .is_err()
    );

    let send = |f: &mut OrdinaryFixture, command| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .native_source_parameter_hold_on_control(source, f.binding, command)
            {
                Ok(token) => break token,
                Err("native preparation is busy" | "native parameter mailbox is busy") => {}
                Err(reason) => panic!("hold refused: {reason}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };
    let receive = |f: &mut OrdinaryFixture, token| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .poll_native_source_parameter_change_on_control(f.binding, token)
            {
                Ok(Progress::Pending) | Err("native preparation is busy") => {}
                Ok(terminal) => break terminal,
                Err(reason) => panic!("hold terminal: {reason}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    };
    let hold = send(&mut f, begin);
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    replacement::wait_for_blocks(&root, f.binding, 2);
    assert!(
        matches!(receive(&mut f, &hold), Progress::Processed(value) if value.sequence == 1 && value.point.id == 7)
    );
    assert_eq!(block(&mut f), [[0.25, -0.5]; 8]);
    replacement::wait_for_blocks(&root, f.binding, 3);
    let release = send(&mut f, Hold::Release { hold: &hold });
    for (index, expected) in [(3, [0.75, -1.5]), (4, [0.75, -1.5]), (5, [0.25, -0.5])] {
        assert_eq!(block(&mut f), [expected; 8]);
        replacement::wait_for_blocks(&root, f.binding, index + 1);
    }
    assert_eq!(
        receive(&mut f, &release),
        Progress::Released {
            sequence: 3,
            first_frame: 24
        }
    );
    assert_eq!(f.editor.project(), &canonical);
    dispose(f);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_track_return_main_controller_queries_preserve_installed_pcm_and_project() {
    use crate::plugin_worker::NativeWorkerControllerProgress;
    use hex_plugin_host::protocol::ParameterPageQuery;
    if std::env::var_os("HEX_ORDINARY_CONTROLLER_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ORDINARY_CONTROLLER_DRIVER", "1")
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
    let _serial = serial();
    for location in 0..3 {
        let mut f = fixture_at("processing", location);
        let canonical = f.editor.project().clone();
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 1);
        let until = Instant::now() + Duration::from_secs(3);
        let query = loop {
            match f.control.begin_native_source_controller_query_on_control(
                f.binding,
                ParameterPageQuery::default(),
            ) {
                Ok(query) => break query,
                Err("native preparation is busy" | "native controller mailbox is busy") => {}
                Err(error) => panic!("ordinary controller admission: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        assert!(
            f.control
                .poll_native_source_controller_query_on_control(
                    Binding {
                        epoch: f.binding.epoch + 1,
                        ..f.binding
                    },
                    &query
                )
                .is_err()
        );
        for n in 1..4_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, n + 1);
            let gain =
                0.5 + n.saturating_sub(2) as f32 * 0.125 + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.25 * gain, -0.5 * gain]; 8]
                }
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        let page = loop {
            match f
                .control
                .poll_native_source_controller_query_on_control(f.binding, &query)
            {
                Ok(NativeWorkerControllerProgress::Page(page)) => break page,
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                }
                other => panic!("ordinary controller completion: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        assert_eq!(
            (
                page.total,
                page.as_slice()[0].id,
                page.as_slice()[0].normalized
            ),
            (1, 7, 0.25)
        );
        assert_eq!(
            page.as_slice()[0].plain,
            hex_plugin_host::protocol::ParameterPlainValue::Finite(1.0)
        );
        let until = Instant::now() + Duration::from_secs(3);
        let conversion = loop {
            match f.control.begin_native_source_controller_query_on_control(
                f.binding,
                ParameterPageQuery {
                    conversion: Some(hex_plugin_host::protocol::ParameterConversion::Parse {
                        id: 7,
                        text: hex_plugin_host::protocol::ParameterText::from_text("double")
                            .unwrap(),
                    }),
                    ..ParameterPageQuery::default()
                },
            ) {
                Ok(query) => break query,
                Err("native preparation is busy" | "native controller mailbox is busy") => {}
                Err(error) => panic!("ordinary SDK parse admission: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .poll_native_source_controller_query_on_control(f.binding, &conversion)
            {
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                }
                Ok(NativeWorkerControllerProgress::Conversion(value)) => {
                    assert_eq!(
                        (value.id, value.normalized, value.plain),
                        (
                            7,
                            Some(0.5),
                            hex_plugin_host::protocol::ParameterPlainValue::Finite(2.0)
                        )
                    );
                    break;
                }
                other => panic!("ordinary SDK parse completion: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(initial)
        );
        assert_eq!(f.editor.project(), &canonical);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        dispose(f);
    }
}

fn begin(f: &mut OrdinaryFixture) -> NativeWorkerCapture {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match f
            .control
            .begin_native_source_state_capture_on_control(f.binding)
        {
            Ok(request) => return request,
            Err("native preparation is busy" | "native capture mailbox is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            Err(reason) => panic!("live capture admission: {reason}"),
        }
    }
}
fn captured(f: &mut OrdinaryFixture, request: &NativeWorkerCapture) -> NativeDeviceState {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match f
            .control
            .poll_native_source_state_capture_on_control(f.binding, request)
        {
            Ok(NativeWorkerCaptureProgress::Captured(state)) => return state,
            Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            other => panic!("live capture terminal: {other:?}"),
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_track_return_main_live_capture_keeps_graph_pcm_and_canonical_state() {
    let _serial = serial();
    for location in 0..3 {
        let mut f = fixture_at("processing", location);
        let canonical = f.editor.project().clone();
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, 1);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        let request = begin(&mut f);
        for n in 1..4_u64 {
            let output = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, n + 1);
            let gain =
                0.5 + n.saturating_sub(2) as f32 * 0.125 + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.25 * gain, -0.5 * gain]; 8]
                }
            );
        }
        let current = captured(&mut f, &request);
        assert_ne!(current, initial);
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(initial)
        );
        assert_eq!(f.editor.project(), &canonical);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        drop(request);
        dispose(f);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_live_capture_held_sdk_keeps_installed_source_and_pcm() {
    if std::env::var_os("HEX_ROOT_LIVE_CAPTURE_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let path =
            std::env::temp_dir().join(format!("hex-root-live-capture-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env("HEX_ROOT_LIVE_CAPTURE_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "71")
            .env("HEX_ORIGINAL_INSTANCE_TRACE", path.join("trace"))
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", path.join("release"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _serial = serial();
    let mut f = fixture("processing");
    let canonical = f.editor.project().clone();
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    block(&mut f);
    replacement::wait_for_blocks(&root, f.binding, 1);
    let applied = f.control.status.native_effect_source().unwrap();
    let request = begin(&mut f);
    let until = Instant::now() + Duration::from_secs(2);
    let trace = std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap();
    while std::fs::read(&trace)
        .ok()
        .is_none_or(|data| data.get(..4) != Some(&22_i32.to_ne_bytes()))
    {
        assert!(Instant::now() < until, "actual SDK capture hold");
        std::thread::yield_now();
    }
    for n in 1..4_u64 {
        assert!(matches!(
            f.control
                .poll_native_source_state_capture_on_control(f.binding, &request)
                .unwrap(),
            NativeWorkerCaptureProgress::Pending
        ));
        let output = block(&mut f);
        replacement::wait_for_blocks(&root, f.binding, n + 1);
        let gain = 0.5 + n.saturating_sub(2) as f32 * 0.125;
        assert_eq!(
            output,
            if n < 2 {
                [[0.0; 2]; 8]
            } else {
                [[0.25 * gain, -0.5 * gain]; 8]
            }
        );
        assert_eq!(f.control.status.native_effect_source(), Some(applied));
    }
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"release",
    )
    .unwrap();
    captured(&mut f, &request);
    assert_eq!(f.editor.project(), &canonical);
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );
    drop(request);
    dispose(f);
}
