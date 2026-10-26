//! Public live capture through actual ordinary graph and installed endpoint.
use super::*;
use crate::plugin_worker::{NativeWorkerCapture, NativeWorkerCaptureProgress};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_track_return_main_host_input_capture_and_commit_reuse_original_source() {
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
        // Keep actual source audio present beyond the handoff. The standard
        // fixture ends at frame32 and its pipeline becomes silent at frame48.
        let clip = f.editor.project().tracks()[0].clips()[0].id();
        f.editor
            .execute(Command::SetClipLooping {
                clip,
                looping: true,
            })
            .unwrap();
        f.editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: 128,
                source_offset: 0,
                source_length: 32,
            })
            .unwrap();
        let canonical = f.editor.project().clone();
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        accepted_state_matches(&f, &initial, true);
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
        let until = Instant::now() + Duration::from_secs(3);
        let snapshot = loop {
            match f
                .control
                .poll_native_source_state_snapshot_on_control(f.binding, &capture)
            {
                Ok(crate::plugin_worker::NativeWorkerStateSnapshotProgress::Captured(snapshot)) => {
                    break snapshot;
                }
                Ok(crate::plugin_worker::NativeWorkerStateSnapshotProgress::Pending)
                | Err("native preparation is busy") => {}
                other => panic!("ordinary SDK snapshot: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .matches_native_source_state_snapshot_on_control(f.binding, &snapshot)
            {
                Ok(true) => break,
                Err("native preparation is busy" | "native capture mailbox is busy") => {}
                other => panic!("ordinary snapshot source: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(
            f.control
                .matches_native_source_state_snapshot_on_control(
                    Binding {
                        epoch: f.binding.epoch + 1,
                        ..f.binding
                    },
                    &snapshot
                )
                .is_err()
        );
        assert!(
            f.control
                .poll_native_source_state_capture_on_control(f.binding, &capture)
                .is_err()
        );
        assert_eq!(
            &hex_plugin_state::decode(snapshot.state())
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
            Some(initial.clone())
        );
        accepted_state_matches(&f, snapshot.state(), false);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        // No caller-supplied envelope can authorize this source. Prepare-before-
        // commit must refuse the captured state until the original capture is staged.
        let device = f
            .editor
            .project()
            .tracks()
            .iter()
            .flat_map(|track| track.devices())
            .chain(
                f.editor
                    .project()
                    .return_tracks()
                    .iter()
                    .flat_map(|bus| bus.devices()),
            )
            .chain(f.editor.project().main_devices())
            .find(|device| device.id().value() == f.binding.instance)
            .unwrap()
            .id();
        let command = Command::ReplaceDeviceState {
            device,
            state: snapshot.state().clone(),
        };
        let refused = f.editor.execute_prepared(command.clone(), |project| {
            compile_captured_project(&f.control, f.binding, project, &f.media, &f.resources)
        });
        assert!(refused.is_err());
        assert_eq!(f.editor.project(), &canonical);
        snapshot_action(|| {
            f.control
                .stage_native_source_state_snapshot_on_control(f.binding, &snapshot)
        })
        .unwrap();
        assert_eq!(
            snapshot_action(|| f
                .control
                .native_source_matches_accepted_state_on_control(f.binding, snapshot.state())
                .map(|_| ())),
            Err("native state approval is pending"),
            "pending bytes are not an accepted save or unchanged-edit baseline"
        );
        assert!(
            snapshot_action(|| f
                .control
                .stage_native_source_state_snapshot_on_control(f.binding, &snapshot))
            .is_err()
        );
        assert!(
            matches!(
                f.control
                    .begin_native_source_parameter_change_on_control(f.binding, 7, 0.5),
                Err("native state approval is pending")
            ),
            "frozen capture cannot admit a later host point before its terminal"
        );
        let (_, candidate) = f
            .editor
            .execute_prepared(command, |project| {
                compile_captured_project(&f.control, f.binding, project, &f.media, &f.resources)
            })
            .unwrap();
        let committed = f.editor.project().clone();
        assert_eq!(committed.device(device).unwrap().state(), snapshot.state());
        let revision = publish(&mut f.control, candidate, PublicationPolicy::InheritRuntime);
        for n in 5..8_u64 {
            let output = block(&mut f);
            assert_eq!(
                f.control.status.applied_revision.load(Acquire),
                revision,
                "matching captured canonical graph must actually install"
            );
            replacement::wait_for_blocks(&root, f.binding, n + 1);
            let gain = 3.0 * (0.5 + (n - 2) as f32 * 0.125) + if location == 1 { 1.0 } else { 0.0 };
            assert_eq!(
                output,
                [[0.25 * gain, -0.5 * gain]; 8],
                "captured graph keeps original worker/PCM prefix"
            );
        }
        snapshot_action(|| {
            f.control
                .finalize_native_source_state_snapshot_on_control(f.binding, &snapshot, true)
        })
        .unwrap();
        accepted_state_matches(&f, snapshot.state(), true);
        accepted_state_matches(&f, &initial, false);
        assert!(
            snapshot_action(|| f
                .control
                .finalize_native_source_state_snapshot_on_control(f.binding, &snapshot, true))
            .is_err()
        );
        // A later capture cannot steal an earlier terminal. Refusing its
        // pending compiler permission preserves the last accepted state.
        let later_request = begin(&mut f);
        let until = Instant::now() + Duration::from_secs(3);
        let later = loop {
            match f
                .control
                .poll_native_source_state_snapshot_on_control(f.binding, &later_request)
            {
                Ok(crate::plugin_worker::NativeWorkerStateSnapshotProgress::Captured(snapshot)) => {
                    break snapshot;
                }
                Ok(crate::plugin_worker::NativeWorkerStateSnapshotProgress::Pending)
                | Err("native preparation is busy") => {}
                other => panic!("later snapshot terminal: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        snapshot_action(|| {
            f.control
                .stage_native_source_state_snapshot_on_control(f.binding, &later)
        })
        .unwrap();
        assert!(
            snapshot_action(|| f
                .control
                .finalize_native_source_state_snapshot_on_control(f.binding, &snapshot, true))
            .is_err()
        );
        // The active canonical bytes still compile while another state is pending.
        drop(
            compile_captured_project(
                &f.control,
                f.binding,
                f.editor.project(),
                &f.media,
                &f.resources,
            )
            .unwrap(),
        );
        snapshot_action(|| {
            f.control
                .finalize_native_source_state_snapshot_on_control(f.binding, &later, false)
        })
        .unwrap();
        accepted_state_matches(&f, snapshot.state(), true);
        assert!(
            snapshot_action(|| f
                .control
                .stage_native_source_state_snapshot_on_control(f.binding, &snapshot))
            .is_err()
        );
        drop(
            compile_captured_project(
                &f.control,
                f.binding,
                f.editor.project(),
                &f.media,
                &f.resources,
            )
            .unwrap(),
        );
        assert_eq!(f.editor.project(), &committed);
        // History contains one actual opaque edit; SDK work did not create extra edges.
        assert!(f.editor.undo());
        assert_eq!(f.editor.project(), &canonical);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &committed);
        assert_eq!(
            f.control.native_source_endpoint_status(f.binding),
            Some((true, true))
        );
        let mut completed = 8u64;
        for epoch in 0..2 {
            if epoch == 0 {
                snapshot_action(|| {
                    f.control
                        .stage_native_source_state_snapshot_on_control(f.binding, &later)
                })
                .unwrap();
            }
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
            // The preceding live handoff may already occupy the two-slot
            // return carrier. Alternate the actual control drain and callback
            // frontier until the original endpoint is ingested; control-only
            // polling cannot advance a callback-held return.
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
            assert!(f.control.status.native_effect_source().is_none());
            if epoch == 0 {
                assert_eq!(
                    snapshot_action(|| f
                        .control
                        .quiesce_native_source_preparation(f.binding, f.binding.graph)
                        .map(|_| ())),
                    Err("native epoch requires state approval terminal")
                );
                snapshot_action(|| {
                    f.control
                        .finalize_native_source_state_snapshot_on_control(f.binding, &later, false)
                })
                .expect("stop must retain the exact pending terminal without reviving transport");
                assert_eq!(
                    f.control.native_source_endpoint_status(f.binding),
                    Some((false, false))
                );
            }
            accepted_state_matches(&f, snapshot.state(), true);
            let previous = f.binding;
            renew_cohort(&mut f);
            accepted_state_matches(&f, snapshot.state(), true);
            accepted_state_matches(&f, &initial, false);
            assert!(
                f.control
                    .native_source_matches_accepted_state_on_control(previous, snapshot.state())
                    .is_err(),
                "matching bytes cannot make a prior source binding current"
            );
            assert_eq!(
                (f.binding.root, f.binding.instance),
                (previous.root, previous.instance)
            );
            assert!(f.binding.epoch > previous.epoch);
            assert!(
                f.control
                    .stage_native_source_state_snapshot_on_control(f.binding, &snapshot)
                    .is_err(),
                "old worker capture cannot authorize a resumed epoch"
            );
            let playback = compile(&f, true);
            let installed = publish(
                &mut f.control,
                playback,
                PublicationPolicy::InstallUnderNativeEpoch(fence),
            );
            assert_eq!(block(&mut f), [[0.0; 2]; 8]);
            assert_eq!(f.control.status.applied_revision.load(Acquire), installed);
            f.control.release_stopped_edit_fence(fence);
            f.control.command(PlaybackCommand::Play).unwrap();
            for n in 0..4u64 {
                let output = block(&mut f);
                replacement::wait_for_blocks(&root, f.binding, completed + n + 1);
                let gain = 3.0 * (0.5 + (completed + n.saturating_sub(2)) as f32 * 0.125)
                    + if location == 1 { 1.0 } else { 0.0 };
                assert_eq!(
                    output,
                    if n < 2 {
                        [[0.0; 2]; 8]
                    } else {
                        [[0.25 * gain, -0.5 * gain]; 8]
                    },
                    "same native lease must retain edited value and actual DSP prefix across resume"
                );
            }
            completed += 4;
            assert_eq!(f.editor.project(), &committed);
        }
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.terminal.is_some_and(|terminal| terminal.blocks == completed))
        );
    }
}

fn snapshot_action(mut run: impl FnMut() -> Result<(), &'static str>) -> Result<(), &'static str> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match run() {
            Err("native preparation is busy" | "native capture mailbox is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            result => return result,
        }
    }
}
fn accepted_state_matches(f: &OrdinaryFixture, state: &NativeDeviceState, expected: bool) {
    snapshot_action(|| {
        f.control
            .native_source_matches_accepted_state_on_control(f.binding, state)
            .map(|matches| assert_eq!(matches, expected))
    })
    .unwrap();
}
fn compile_captured_project(
    control: &Control,
    binding: Binding,
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, hex_engine::Audio>,
    resources: &PreparedDeviceResources,
) -> Result<Playback, hex_project::Invalid> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = crate::plugin_effects::Hosting::prepare_on_control(
            control,
            &[binding],
            &[],
            &mut |effects| {
                effects.prepare_registered_revision(
                    project,
                    media,
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                    &hex_engine::audio_transform::AudioTransformPreparation::default(),
                    resources,
                    1_000_000,
                    &AtomicBool::new(false),
                )
            },
        );
        match result {
            Ok(revision) => {
                return Playback::try_from_prepared_with_device_resources(
                    Arc::new(revision),
                    resources,
                )
                .map_err(|_| hex_project::Invalid("captured runtime refused"));
            }
            Err(error)
                if matches!(
                    error.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) =>
            {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            Err(_) => return Err(hex_project::Invalid("captured source refused")),
        }
    }
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
