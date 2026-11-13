//! Signed controller eligibility and ordinary Session authoring.
use super::*;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
};

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn fresh_selected_lookup_finds_raw_zero_beyond_the_first_page_without_inventory() {
    const CHILD: &str = "HEX_DESKTOP_SELECTED_LOOKUP_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "80")
            .env("HEX_ORIGINAL_PARAMETER_INVENTORY", "9")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    let _native = native_fixture();
    let (mut output, mut backend) = hex_audio::worker_fixture::open(48_000).unwrap();
    let control = output.publication.control().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 4096,
                root_rows: 128,
                max_retained_bytes: 32 * 1024 * 1024,
                max_construction_peak_bytes: 32 * 1024 * 1024,
            },
        )
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            hex_audio::AudioCandidateDomainReservation {
                max_retained_data_bytes: 192 * 1024 * 1024,
                max_construction_peak_data_bytes: 192 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    control
        .configure_native_source_preparation(
            1,
            hex_audio::NativeSourceDataReservation {
                max_retained_data_bytes: 4 * 1024 * 1024,
                max_construction_peak_data_bytes: 24 * 1024 * 1024,
            },
        )
        .unwrap();
    let host = host();
    let selected = selection();
    let binding = host
        .begin_on_control(
            control,
            hex_audio::plugin_effects::EffectSelection {
                module: &selected.module,
                executable: &selected.executable,
                cid: selected.cid,
                executable_sha256: &selected.executable_sha256,
                info_sha256: &selected.info_sha256,
            },
            PluginEffectDestination::Main.owner(),
            1,
            1,
            48_000,
            hex_audio::plugin_effects::PluginProfile::Effect,
            None,
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match control.native_source_preparation_progress(binding) {
            Some(hex_audio::NativeSourcePreparationProgress::Facts(_)) => host
                .complete_on_control(
                    control,
                    binding,
                    Some(EffectPresentation {
                        input_frames: 0,
                        output_frames: 512,
                    }),
                )
                .unwrap(),
            Some(hex_audio::NativeSourcePreparationProgress::Prepared { .. }) => control
                .start_native_effect_with_authored_selection(binding, None)
                .unwrap(),
            Some(hex_audio::NativeSourcePreparationProgress::Effect { .. }) => break,
            _ => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
        }
    }
    for hint in [None, Some(8), Some(4)] {
        assert_eq!(
            crate::native_audio::plugins::Plugins::lookup_authored_target_for_test(
                control, binding, 0, hint
            )
            .unwrap(),
            (0, 8)
        );
    }
    assert!(
        crate::native_audio::plugins::Plugins::lookup_authored_target_for_test(
            control,
            binding,
            i32::MAX as u32,
            None
        )
        .is_err()
    );
    assert!(
        crate::native_audio::plugins::Plugins::lookup_authored_target_for_test(
            control,
            hex_audio::plugin_effects::EffectBinding {
                epoch: binding.epoch + 1,
                ..binding
            },
            0,
            None
        )
        .is_err()
    );
    control.retire_native_source_preparation(binding);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        control.reclaim();
        if control.recover_native_source_preparation(binding).is_some() {
            break;
        }
        assert!(Instant::now() < until, "lookup source disposal");
        std::thread::yield_now();
    }
    drop(output);
    backend.close();
}

pub(super) fn edit_with_running_callback(h: &mut ResidentHarness, request: Request) -> Event {
    h.service.submit(request).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        // This is a headless callback driver, not a physical device clock.
        // Observe actual native completion before accelerating another quantum;
        // otherwise this control loop can manufacture missed worker deadlines.
        h.render_worker_step();
        if let Ok(event) = h.service.try_event() {
            return event;
        }
        assert!(Instant::now() < until, "vst3 lane edit did not complete");
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn signed_vst3_rows_admit_only_matching_arrangement_lane_edits() {
    const CHILD: &str = "HEX_DESKTOP_TEST_VST3_LANES_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        for variant in ["92", "81", "83", "84"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--ignored", "--nocapture"])
                .env(CHILD, variant)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "variant {variant}\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    }
    let variant = std::env::var(CHILD).unwrap();
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    h.applied(0);
    let insert = request(&initial.state, PluginEffectDestination::Main);
    let exact_class = insert.selection.cid;
    h.run(Request::InsertPluginEffect(insert));
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let (applied, metadata) = h.applied(1);
    let device = accepted.state.project.main_devices()[0].id();
    let query = hex_session::service::PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
        request_id: 1,
        page_first: None,
    };
    assert_eq!(
        h.run(Request::QueryPluginParameters(query)).state.revision,
        accepted.state.revision
    );
    let observed = h.next();
    assert_eq!(observed.state.project, accepted.state.project);
    let observed = observed.plugin_parameters.unwrap();
    let original_binding = observed.source.unwrap();
    let rows = observed.rows.unwrap();
    let [row] = rows.as_slice() else {
        panic!("expected one SDK parameter")
    };
    let eligible = variant == "92" || variant == "81";
    assert_eq!(row.automation_class, eligible.then_some(exact_class));
    assert_eq!(row.editable, eligible);
    assert_eq!(row.id, if variant == "92" { 0 } else { 7 });
    assert_eq!(row.steps, if variant == "81" { 3 } else { 0 });

    let target = DeviceAutomationTarget::Vst3 { parameter: row.id };
    let point = |beat, value, outgoing| AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    };
    let points = vec![
        point(
            BeatTime::ZERO,
            0.0,
            if row.steps == 0 {
                AutomationInterpolation::Linear
            } else {
                AutomationInterpolation::Step
            },
        ),
        point(
            BeatTime::from_ratio(1, 1).unwrap(),
            1.0,
            AutomationInterpolation::Step,
        ),
    ];
    let edit = |revision, vst3_class, points| Request::DeviceAutomation {
        project_generation: metadata.context.project_generation,
        revision,
        device,
        target,
        vst3_class,
        points,
    };
    let revision = accepted.state.revision;
    for refused in [
        edit(revision - 1, Some(exact_class), points.clone()),
        edit(revision, Some([0xa3; 16]), points.clone()),
    ] {
        h.service.submit(refused).unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, accepted.state.project);
        assert_eq!(refused.state.revision, revision);
    }
    let event =
        edit_with_running_callback(&mut h, edit(revision, Some(exact_class), points.clone()));
    if !eligible {
        assert!(
            event.error.is_some(),
            "unsupported SDK parameter was admitted"
        );
        assert_eq!(event.state.project, accepted.state.project);
        assert_eq!(event.state.revision, revision);
        return;
    }
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(event.state.revision, revision + 1);
    let (mut applied, applied_metadata) = h.applied(applied);
    if variant == "92" {
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        let query = hex_session::service::PluginParameterQuery {
            context: applied_metadata.context,
            request_id: 2,
            ..query
        };
        h.run(Request::QueryPluginParameters(query));
        let observed = h.next().plugin_parameters.unwrap();
        let successor = observed.source.unwrap();
        assert_eq!(successor.root, original_binding.root);
        assert_eq!(successor.instance, original_binding.instance);
        assert_ne!(
            successor.graph, original_binding.graph,
            "stopped automation edit replaces the editor's original worker"
        );
    }
    let lane = event
        .state
        .project
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(lane.vst3_class, Some(exact_class));
    assert_eq!(lane.points, points);

    let undone = edit_with_running_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, accepted.state.project);
    applied = h.applied(applied).0;
    let redone = edit_with_running_callback(&mut h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, event.state.project);
    let applied = h.applied(applied).0;

    let files = Files::new();
    let path = files.0.join("authored-vst3.hex");
    hex_storage::save_new(&path, &redone.state.project).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), *redone.state.project);

    if variant == "92" {
        use hex_project::AutomationReadSelection as Read;
        use hex_session::service::DeviceAutomationControlEdit as Edit;
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render();
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        let select = |revision, class, selection| Request::DeviceAutomationControl {
            project_generation: metadata.context.project_generation,
            revision,
            edit: Edit::ReadSelection {
                device,
                target,
                vst3_class: Some(class),
                selection,
            },
        };
        let refused = edit_with_running_callback(
            &mut h,
            select(redone.state.revision, [0x35; 16], Read::Base),
        );
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, redone.state.project);
        assert_eq!(refused.state.revision, redone.state.revision);
        let overridden = edit_with_running_callback(
            &mut h,
            select(redone.state.revision, exact_class, Read::Base),
        );
        assert!(overridden.error.is_none(), "{:?}", overridden.error);
        let mut expected = lane.clone();
        expected.read_selection = Read::Base;
        assert_eq!(
            overridden
                .state
                .project
                .device(device)
                .unwrap()
                .automation_target(target),
            Some(&expected)
        );
        assert_eq!(overridden.state.revision, redone.state.revision + 1);
        let applied = h.applied(applied).0;
        let restored = edit_with_running_callback(
            &mut h,
            select(overridden.state.revision, exact_class, Read::Authored),
        );
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert_eq!(restored.state.project, redone.state.project);
        let applied = h.applied(applied).0;
        let undone = edit_with_running_callback(&mut h, Request::Undo);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.project, overridden.state.project);
        let applied = h.applied(applied).0;
        let redone = edit_with_running_callback(&mut h, Request::Redo);
        assert!(redone.error.is_none(), "{:?}", redone.error);
        assert_eq!(redone.state.project, restored.state.project);
        h.applied(applied);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn eq_creation_commits_and_cancels_after_vst3_worker_receives_the_preview() {
    use hex_engine::eq::{EqState, Node, NodeId};
    use hex_session::device_gesture::{EqNodeCreationSnapshot, GestureKey, GestureRequest};

    const CHILD: &str = "HEX_DESKTOP_EQ_VST3_PREVIEW_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "92")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    assert!(initial.error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::InsertMainDevice {
        before: None,
        state: EqState::default().encode().unwrap(),
    }));
    let Some(Created::Device(eq)) = added.created else {
        panic!("eq device");
    };
    (applied, _) = h.applied(applied);

    let insert = request(&added.state, PluginEffectDestination::Main);
    let class = insert.selection.cid;
    h.run(Request::InsertPluginEffect(insert));
    let accepted = terminal(&mut h);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let plugin = accepted.state.project.main_devices()[1].id();
    let (revision, metadata) = h.applied(applied);
    let generation = metadata.context.project_generation;
    let authored = edit_with_running_callback(
        &mut h,
        Request::DeviceAutomation {
            project_generation: generation,
            revision: accepted.state.revision,
            device: plugin,
            target: DeviceAutomationTarget::Vst3 { parameter: 0 },
            vst3_class: Some(class),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.4).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        },
    );
    assert!(authored.error.is_none(), "{:?}", authored.error);
    (applied, _) = h.applied(revision);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let before = &authored.state.project;
    let begin = |h: &mut ResidentHarness, token, project: &hex_project::Project| {
        let key = GestureKey { generation, token };
        let next = project.next_identity();
        let node = Node::new(NodeId::new(next).unwrap(), 750.0, 12.0, 0).unwrap();
        let draft =
            EqNodeCreationSnapshot::new(node, project.device(eq).unwrap().active_variant(), next)
                .unwrap();
        let event = h.run(Request::DeviceGesture(GestureRequest::BeginEqCreation {
            key,
            device: eq,
            snapshot: draft,
        }));
        assert!(event.gesture_ack.unwrap().accepted);
        (key, draft)
    };
    let render_preview = |h: &mut ResidentHarness, applied, key, sequence, draft| {
        let event = edit_with_running_callback(
            h,
            Request::DeviceGesture(GestureRequest::ChangeEqCreation {
                key,
                sequence,
                snapshot: draft,
            }),
        );
        let ack = event.gesture_ack.unwrap();
        assert!(ack.accepted, "{ack:?}");
        let (applied, metadata) = h.applied(applied);
        assert_eq!(metadata.device_preview.unwrap().sequence, sequence);
        for _ in 0..8 {
            h.render_worker_step();
        }
        applied
    };
    let (key, draft) = begin(&mut h, 1, before);
    applied = render_preview(&mut h, applied, key, 1, draft);
    applied = render_preview(&mut h, applied, key, 2, draft);
    let committed = edit_with_running_callback(
        &mut h,
        Request::DeviceGesture(GestureRequest::CommitEqCreation {
            key,
            sequence: 3,
            snapshot: draft,
        }),
    );
    let ack = committed.gesture_ack.unwrap();
    assert!(ack.accepted, "{ack:?}");
    assert_eq!(committed.state.revision, authored.state.revision + 1);
    assert_eq!(
        EqState::decode(committed.state.project.device(eq).unwrap().state())
            .unwrap()
            .nodes
            .len(),
        1,
    );
    let (revision, metadata) = h.applied(applied);
    assert!(metadata.device_preview.is_none());

    let undone = edit_with_running_callback(&mut h, Request::Undo);
    assert_eq!(undone.state.project.device(eq), before.device(eq));
    (applied, _) = h.applied(revision);
    let redone = edit_with_running_callback(&mut h, Request::Redo);
    assert_eq!(redone.state.project, committed.state.project);
    (applied, _) = h.applied(applied);

    let (key, draft) = begin(&mut h, 2, &redone.state.project);
    applied = render_preview(&mut h, applied, key, 1, draft);
    let cancelled = edit_with_running_callback(
        &mut h,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(cancelled.gesture_ack.unwrap().accepted);
    assert_eq!(cancelled.state.project, redone.state.project);
    assert_eq!(cancelled.state.revision, redone.state.revision);
    let (applied, metadata) = h.applied(applied);
    assert!(metadata.device_preview.is_none());

    let (key, draft) = begin(&mut h, 3, &cancelled.state.project);
    render_preview(&mut h, applied, key, 1, draft);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(h.publication_refusals.is_empty());
}
