//! Actual ordinary command/history terminals with original native sources.
use super::*;

fn with_gain(state: &NativeDeviceState, gain: f32) -> NativeDeviceState {
    with_gain_and_axis(state, gain, None)
}

fn with_gain_and_axis(
    state: &NativeDeviceState,
    gain: f32,
    axis_gain: Option<f64>,
) -> NativeDeviceState {
    let captured = hex_plugin_state::decode(state).unwrap();
    let mut component = captured.component.unwrap().to_vec();
    assert_eq!(&component[..4], b"HXPS");
    component[8..12].copy_from_slice(&gain.to_le_bytes());
    if let Some(axis_gain) = axis_gain {
        component[32..40].copy_from_slice(&axis_gain.to_le_bytes());
    }
    let mut controller = captured.controller.map(<[u8]>::to_vec);
    if let Some(controller) = controller.as_mut() {
        assert_eq!(&controller[..4], b"HXCT");
        controller[4..8].copy_from_slice(&91_u32.to_le_bytes());
    }
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: captured.identity,
        component: Some(&component),
        controller: controller.as_deref(),
    })
    .unwrap()
}
pub(super) fn ordinary_terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        // The headless driver advances only after the actual installed workers
        // have completed this callback's request prefixes. Tight-loop rendering
        // while native preparation runs would manufacture missed deadlines.
        workers_idle(h);
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            return event;
        }
        assert!(Instant::now() < end, "ordinary native edit terminal");
        std::thread::yield_now();
    }
}
fn workers_idle(h: &ResidentHarness) {
    wait(|| {
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .native_workers_idle_on_control()
            == Some(true)
    });
}

fn hold_graph_publication(
    h: &mut ResidentHarness,
    request: Request,
) -> (mpsc::SyncSender<()>, std::thread::JoinHandle<()>) {
    let (ready, resume, busy) = h.probe_next_native_publication();
    h.service.submit(request).unwrap();
    graph_publication_ready(h, &ready);
    let (held, observed_hold) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let holder = h.hold_native_work_on_control(held, released);
    observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
    resume.send(()).unwrap();
    if let Err(error) = busy.recv_timeout(Duration::from_secs(10)) {
        release.send(()).unwrap();
        holder.join().unwrap();
        panic!("actual Root graph Work refusal: {error}");
    }
    (release, holder)
}
fn graph_publication_ready(h: &mut ResidentHarness, ready: &mpsc::Receiver<()>) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.try_recv().is_ok() {
            return;
        }
        h.pump();
        h.render();
        workers_idle(h);
        assert!(Instant::now() < until, "actual compiled graph publication");
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn plugin_editor_close_releases_unobserved_holds_before_next_attempt() {
    use crate::native_audio::tests::resident_preview::emit_replay_fixture_edit;
    use hex_session::service::{
        DeviceAutomationTakeProgress, PluginEditorCloseAdmission as Admission,
        PluginEditorCloseAttempt,
    };

    const CHILD: &str = "HEX_8291_CLOSE_HOLD_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let release = files.0.join("release");
        let traces = files.0.join("traces");
        fs::create_dir_all(&release).unwrap();
        fs::create_dir_all(&traces).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "147")
            .env("HEX_ORIGINAL_EDITOR_RELEASE_DIR", release)
            .env("HEX_ORIGINAL_NOTE_TRACE_DIR", traces)
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

    let _native = native_fixture();
    let (mut h, files, saved) = fixture(Start::Saved);
    h.applied(0);
    let context = saved.canonical_audio_source.as_ref().unwrap().context();
    assert_eq!(context.editor_revision, saved.revision);
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    let traces = PathBuf::from(std::env::var_os("HEX_ORIGINAL_NOTE_TRACE_DIR").unwrap());
    let trace = fs::read_dir(traces)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "bin"))
        .unwrap();
    emit_replay_fixture_edit(&trace);

    let mut request_id = 0;
    let mut prepare = |h: &mut ResidentHarness, context, attempt| {
        request_id += 1;
        h.service
            .submit(Request::PreparePluginEditorClose {
                request_id,
                context,
                attempt,
            })
            .unwrap();
        loop {
            let event = h.next();
            assert!(event.error.is_none(), "{:?}", event.error);
            if let Some(result) = event.device_automation_take_complete
                && result.request_id == request_id
            {
                let DeviceAutomationTakeProgress::ClosePluginEditor(admission) = result.progress
                else {
                    panic!("unexpected close response: {:?}", result.progress);
                };
                return admission;
            }
        }
    };
    let first = Arc::new(PluginEditorCloseAttempt::default());
    let first_request = Arc::downgrade(&first);
    let stale = hex_engine::AudioSourceContext {
        project_generation: context.project_generation + 1,
        ..context
    };
    assert_eq!(
        prepare(&mut h, stale, first_request.clone()),
        Admission::Refused
    );
    assert_eq!(
        prepare(&mut h, context, first_request.clone()),
        Admission::Pending
    );
    drop(first);
    assert_eq!(prepare(&mut h, context, first_request), Admission::Refused);

    // A second attempt uses another token. The same SDK will refuse it while
    // the first hold remains installed, even when its caller never saw Held.
    let mut acquire = || {
        let attempt = Arc::new(PluginEditorCloseAttempt::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match prepare(&mut h, context, Arc::downgrade(&attempt)) {
                Admission::Pending => {}
                Admission::Held { admitted_edits } => {
                    assert!(
                        admitted_edits,
                        "the original SDK input frontier survives release"
                    );
                    break;
                }
                Admission::Refused => panic!("the original worker refused a fresh hold"),
            }
            assert!(Instant::now() < deadline, "cancelled close leaked its hold");
        }
        assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
        attempt
    };
    drop(acquire());
    let held = acquire();
    assert!(held.is_valid());
    h.service.submit(Request::SaveCurrent).unwrap();
    let save = ordinary_terminal(&mut h);
    assert!(
        save.save_complete && save.error.is_none(),
        "{:?}",
        save.error
    );
    let stored = hex_storage::load(files.0.join("effect.hex")).unwrap();
    let state = hex_plugin_state::decode(stored.main_devices()[0].state()).unwrap();
    assert_eq!(
        f32::from_le_bytes(state.component.unwrap()[8..12].try_into().unwrap()),
        3.0,
        "save under the admission hold captures the original SDK change"
    );
    assert!(held.is_valid());

    // Replacing the exact native source invalidates the close decision even
    // though its original caller keeps the grant alive through a prompt.
    let device = &saved.project.main_devices()[0];
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device: device.id(),
            state: with_gain(device.state(), 0.75),
        }))
        .unwrap();
    let event = ordinary_terminal(&mut h);
    assert!(event.error.is_none(), "{:?}", event.error);
    let deadline = Instant::now() + Duration::from_secs(2);
    while held.is_valid() {
        h.pump();
        h.render_worker_step();
        assert!(
            Instant::now() < deadline,
            "replacement left the old close decision valid"
        );
        std::thread::yield_now();
    }
    assert_eq!(hex_audio::audio_lifecycle_failure(), None);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_receipts_survive_replacement_and_callback_held_shutdown() {
    retiring_editor_replay_roundtrip(ReplayCase::Conflict);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_replay_saves_and_reopens_on_the_same_successor() {
    retiring_editor_replay_roundtrip(ReplayCase::Publish);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_already_in_canonical_successor_releases_original_receipt() {
    retiring_editor_replay_roundtrip(ReplayCase::AlreadyCanonical);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn pre_cut_editor_in_canonical_successor_releases_original_receipt() {
    retiring_editor_replay_roundtrip(ReplayCase::PreCutAlreadyCanonical);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn pre_cut_editor_conflict_preserves_the_replacement_and_original_receipt() {
    retiring_editor_replay_roundtrip(ReplayCase::PreCutConflict);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn earlier_group_matching_both_values_releases_the_original_receipt() {
    retiring_editor_replay_roundtrip(ReplayCase::EarlierGrouped);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn earlier_group_with_one_conflicting_value_stays_in_original_custody() {
    retiring_editor_replay_roundtrip(ReplayCase::EarlierGroupedConflict);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn group_open_at_preparation_confirms_earlier_values_after_its_terminal() {
    retiring_editor_replay_roundtrip(ReplayCase::SpanningGroup);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn group_open_at_preparation_keeps_the_receipt_when_one_earlier_value_conflicts() {
    retiring_editor_replay_roundtrip(ReplayCase::SpanningGroupConflict);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn mixed_retiring_editor_replays_group_after_confirming_earlier_value() {
    retiring_editor_replay_roundtrip(ReplayCase::Mixed);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn mixed_retiring_editor_keeps_later_group_unapplied_if_earlier_value_conflicts() {
    retiring_editor_replay_roundtrip(ReplayCase::MixedConflict);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_group_replays_both_values_and_reopens_on_the_same_successor() {
    retiring_editor_replay_roundtrip(ReplayCase::Grouped);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn later_vst3_cohorts_replay_in_order_and_save_final_values() {
    retiring_editor_replay_roundtrip(ReplayCase::SeveralGroups);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn later_vst3_cohorts_restore_original_values_when_publication_refuses() {
    retiring_editor_replay_roundtrip(ReplayCase::SeveralGroupsRefuse);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_replay_preserves_playing_transport() {
    retiring_editor_replay_roundtrip(ReplayCase::Playing);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_replay_refusal_restores_confirmed_successor() {
    retiring_editor_replay_roundtrip(ReplayCase::Refuse);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn late_editor_replay_refusal_preserves_newer_editor_input() {
    retiring_editor_replay_roundtrip(ReplayCase::RefuseAfterNewer);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayCase {
    Conflict,
    Publish,
    AlreadyCanonical,
    PreCutAlreadyCanonical,
    PreCutConflict,
    EarlierGrouped,
    EarlierGroupedConflict,
    SpanningGroup,
    SpanningGroupConflict,
    Mixed,
    MixedConflict,
    Grouped,
    SeveralGroups,
    SeveralGroupsRefuse,
    Playing,
    Refuse,
    RefuseAfterNewer,
}

fn retiring_editor_replay_roundtrip(case: ReplayCase) {
    const CHILD: &str = "HEX_RETIRING_SOURCE_REPLACEMENT";
    let mixed = matches!(case, ReplayCase::Mixed | ReplayCase::MixedConflict);
    let spanning = matches!(
        case,
        ReplayCase::SpanningGroup | ReplayCase::SpanningGroupConflict
    );
    let earlier_group = spanning
        || matches!(
            case,
            ReplayCase::EarlierGrouped | ReplayCase::EarlierGroupedConflict
        );
    let several_groups = matches!(
        case,
        ReplayCase::SeveralGroups | ReplayCase::SeveralGroupsRefuse
    );
    let grouped = earlier_group || mixed || several_groups || case == ReplayCase::Grouped;
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let release = files.0.join("release");
        let traces = files.0.join("traces");
        fs::create_dir_all(&release).unwrap();
        fs::create_dir_all(&traces).unwrap();
        let outcome = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .envs(
                matches!(
                    case,
                    ReplayCase::Refuse
                        | ReplayCase::RefuseAfterNewer
                        | ReplayCase::SeveralGroupsRefuse
                )
                .then_some(("HEX_REPLAY_TEST_REFUSE", "1")),
            )
            .envs(mixed.then_some(("HEX_EXPECT_RETIRING_MIXED_BATCH", "1")))
            .envs(several_groups.then_some(("HEX_EXPECT_RETIRING_ORDERED_COHORTS", "1")))
            .envs(earlier_group.then_some((
                "HEX_EXPECT_RETIRING_EARLIER_GROUP",
                if spanning { "spanning" } else { "complete" },
            )))
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if grouped { "148" } else { "116" },
            )
            .env(
                "HEX_ORIGINAL_GROUPED_EDITOR_PATTERN",
                if mixed {
                    "ordinary-first"
                } else if spanning {
                    "open-at-preparation"
                } else if several_groups {
                    "repeat"
                } else {
                    "split"
                },
            )
            .env(
                "HEX_ORIGINAL_GROUPED_RELEASE",
                files.0.join("group-release"),
            )
            .env("HEX_ORIGINAL_EDITOR_END_RELEASE", files.0.join("end"))
            .env("HEX_ORIGINAL_EDITOR_RELEASE_DIR", release)
            .env("HEX_ORIGINAL_NOTE_TRACE_DIR", traces)
            .output()
            .unwrap();
        assert!(
            outcome.status.success()
                && String::from_utf8_lossy(&outcome.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&outcome.stdout),
            String::from_utf8_lossy(&outcome.stderr)
        );
        return;
    }

    let _native = native_fixture();
    let playing = case == ReplayCase::Playing;
    let pre_cut = matches!(
        case,
        ReplayCase::PreCutAlreadyCanonical | ReplayCase::PreCutConflict
    );
    let already_canonical = matches!(
        case,
        ReplayCase::AlreadyCanonical
            | ReplayCase::PreCutAlreadyCanonical
            | ReplayCase::EarlierGrouped
            | ReplayCase::SpanningGroup
    );
    let effect_host = || {
        let host = host();
        if grouped {
            host.with_grouped_editor_callbacks()
        } else {
            host
        }
    };
    let frames = if playing { 48_000 * 20 } else { 2048 };
    let (mut h, files, saved) = fixture_with_host(Start::Saved, frames, effect_host());
    let (applied, _) = h.applied(0);
    let original = &saved.project.main_devices()[0];
    let device = original.id();
    let replacement_gain = if already_canonical || earlier_group {
        3.0
    } else if case == ReplayCase::Mixed {
        2.0
    } else {
        1.5
    };
    let replacement = with_gain_and_axis(
        original.state(),
        replacement_gain,
        (earlier_group && already_canonical).then_some(2.0),
    );
    let old_binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    let traces_dir = PathBuf::from(std::env::var_os("HEX_ORIGINAL_NOTE_TRACE_DIR").unwrap());
    let mut traces = fs::read_dir(&traces_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bin"));
    let old_trace = traces.next().expect("original SDK helper trace");
    assert!(traces.next().is_none());
    use crate::native_audio::tests::resident_preview::emit_replay_fixture_edit as emit;
    let finish_group = || {
        fs::write(
            std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
            b"end group",
        )
        .unwrap();
    };
    if pre_cut || earlier_group {
        emit(&old_trace);
    }
    if earlier_group && !spanning {
        finish_group();
    }
    let release = PathBuf::from(std::env::var_os("HEX_ORIGINAL_EDITOR_RELEASE_DIR").unwrap());
    if mixed {
        fs::write(
            release.join(old_trace.file_stem().unwrap()),
            b"ordinary edit",
        )
        .unwrap();
    }
    let earlier_frontier = if mixed {
        Some(3)
    } else if spanning {
        Some(5)
    } else if earlier_group {
        Some(12)
    } else {
        None
    };
    if let Some(expected) = earlier_frontier {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let frontier = super::super::editor_recording::probe(&h, |reply| {
                crate::native_audio::tests::resident_preview::EditorRecordingProbe::EditorFrontier(
                    old_binding,
                    reply,
                )
            });
            if frontier == Some(expected) {
                break;
            }
            assert!(
                Instant::now() < until,
                "original editor frontier {expected} was not observed"
            );
        }
    }
    if playing {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render_plugin_block();
        workers_idle(&h);
    }
    let (ready, resume, _) = h.probe_next_native_publication();
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        }))
        .unwrap();
    graph_publication_ready(&mut h, &ready);
    if mixed {
        fs::write(
            std::env::var_os("HEX_ORIGINAL_GROUPED_RELEASE").unwrap(),
            b"grouped edit",
        )
        .unwrap();
    }
    if !pre_cut && !earlier_group {
        emit(&old_trace);
    }
    if grouped && (!earlier_group || spanning) {
        finish_group();
    }
    if matches!(case, ReplayCase::Conflict | ReplayCase::RefuseAfterNewer) {
        let successor_trace = fs::read_dir(&traces_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path != &old_trace && path.extension().is_some_and(|ext| ext == "bin"))
            .expect("prepared successor trace");
        if case == ReplayCase::Conflict {
            emit(&successor_trace);
        } else {
            fs::write(
                release.join("after-replay"),
                successor_trace.to_str().unwrap(),
            )
            .unwrap();
        }
    }
    resume.send(()).unwrap();
    let accepted = ordinary_terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(
        accepted
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .processing_eq(&replacement)
    );
    let (replacement_audio, _) = h.applied(applied);
    let installed = |h: &ResidentHarness| {
        h.plugin_progress
            .lock()
            .unwrap()
            .source_bindings
            .into_iter()
            .flatten()
            .find_map(|(id, binding, _)| (id == device.value()).then_some(binding))
    };
    h.pump();
    let start = Instant::now();
    let successor = loop {
        if let Some(binding) = installed(&h)
            && binding != old_binding
        {
            break binding;
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        h.pump();
        std::thread::yield_now();
    };

    if already_canonical {
        use hex_session::service::{
            DeviceAutomationTakeProgress as Progress, RetiringEditorCloseBlocker,
        };

        let mut request_id = 1;
        let settled = loop {
            h.service
                .submit(Request::QueryDeviceAutomationCloseSafety { request_id })
                .unwrap();
            let event = ordinary_terminal(&mut h);
            assert!(event.error.is_none(), "{:?}", event.error);
            let safety = event.device_automation_take_complete.unwrap();
            assert_eq!(safety.request_id, request_id);
            match safety.progress {
                Progress::CloseSafe => break event.state,
                Progress::CloseRetiringEditor(RetiringEditorCloseBlocker::Transferring) => {}
                other => panic!("same-value replay failed to settle: {other:?}"),
            }
            assert!(start.elapsed() < Duration::from_secs(2));
            request_id += 1;
        };
        assert_eq!(settled.revision, accepted.state.revision);
        assert_eq!(settled.project, accepted.state.project);
        assert_eq!(settled.unsaved, accepted.state.unsaved);
        assert_eq!(installed(&h), Some(successor));
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .recording_source()
                .unwrap()
                .audio_revision,
            replacement_audio
        );
        use crate::native_audio::tests::resident_preview::EditorRecordingProbe;
        assert!(
            super::super::editor_recording::probe(&h, |reply| {
                EditorRecordingProbe::RetiringBatch(old_binding, reply)
            })
            .is_none()
        );
        let saved = h.run(Request::SaveCurrent);
        assert!(saved.save_complete);
        assert_eq!(saved.state.revision, accepted.state.revision);
        let disk = hex_storage::load(files.0.join("effect.hex")).unwrap();
        assert_eq!(disk.device(device).unwrap().state(), &replacement);
        return;
    }

    if matches!(
        case,
        ReplayCase::Publish
            | ReplayCase::Playing
            | ReplayCase::Grouped
            | ReplayCase::Mixed
            | ReplayCase::SeveralGroups
    ) {
        let event = loop {
            h.pump();
            h.render_worker_step();
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if event.state.revision > accepted.state.revision {
                    break event;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "canonical replay did not publish, {:?}",
                h.retirement_issues
            );
            std::thread::yield_now();
        };
        let (_, publication) = h.applied(replacement_audio);
        assert_eq!(publication.context.editor_revision, event.state.revision);
        assert_eq!(event.state.revision, accepted.state.revision + 1);
        assert_eq!(installed(&h), Some(successor));
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire),
            playing
        );
        let captured =
            hex_plugin_state::decode(event.state.project.device(device).unwrap().state()).unwrap();
        let final_gain = if several_groups { 1.0 } else { 3.0 };
        assert_eq!(
            f32::from_le_bytes(captured.component.unwrap()[8..12].try_into().unwrap()),
            final_gain
        );
        let saved = h.run(Request::SaveCurrent);
        assert!(
            saved.save_complete && saved.error.is_none(),
            "{:?}",
            saved.error
        );
        let path = files.0.join("effect.hex");
        let saved_project = hex_storage::load(&path).unwrap();
        let state =
            hex_plugin_state::decode(saved_project.device(device).unwrap().state()).unwrap();
        assert_eq!(
            f32::from_le_bytes(state.component.unwrap()[8..12].try_into().unwrap()),
            final_gain
        );
        if grouped {
            for capture in [captured, state] {
                assert_eq!(
                    f64::from_le_bytes(capture.component.unwrap()[32..40].try_into().unwrap()),
                    if several_groups { 1.0 } else { 2.0 }
                );
            }
        }
        let previous_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.service.shutdown(Duration::from_secs(1)).unwrap();
        assert!(
            h.callback
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .shutdown_requested()
        );
        assert_eq!(hex_audio::audio_lifecycle_failure(), None);
        h.release_relinquished_output();
        drop(h);
        let mut selected = selection();
        let module = files.0.canonicalize().unwrap().join("original.vst3");
        ui::copy_original_module(&selected.module, &module);
        selected.module = module;
        hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Fx", 0).unwrap();
        let mut h = ResidentHarness::with_plugins_and_catalog(
            effect_host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            files.0.canonicalize().unwrap().join("plugin-catalog.json"),
        );
        assert!(h.next().error.is_none());
        h.applied(0);
        let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        assert_ne!(output, previous_output);
        let opened = h.run(Request::Open(path));
        assert!(opened.open_complete && opened.project_replaced);
        assert_eq!(opened.state.project.as_ref(), &saved_project);
        super::super::resident_lifecycle::finish_replacement(&mut h, &opened, output);
        let restored = h.next();
        assert!(restored.error.is_none() && !restored.request_complete);
        let mut serial = 0;
        let rows = super::super::installed_edits::query(&mut h, device, None, &mut serial)
            .rows
            .unwrap();
        let gain_id = if grouped { 0 } else { 7 };
        assert_eq!(
            rows.iter()
                .find(|row| row.id == gain_id)
                .unwrap()
                .normalized,
            if several_groups { 0.25 } else { 0.75 }
        );
        if grouped {
            assert_eq!(
                rows.iter().find(|row| row.id == 11).unwrap().normalized,
                if several_groups { 0.25 } else { 0.5 }
            );
        }
        assert_eq!(hex_audio::audio_lifecycle_failure(), None);
        return;
    }

    let error = loop {
        h.pump();
        h.render_worker_step();
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.revision, accepted.state.revision);
            assert_eq!(event.state.project, accepted.state.project);
            if let Some(error) = event.error {
                break error;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "missing replay refusal"
        );
        std::thread::yield_now();
    };
    if matches!(
        case,
        ReplayCase::Refuse | ReplayCase::RefuseAfterNewer | ReplayCase::SeveralGroupsRefuse
    ) {
        assert!(error.contains("fixture final admission refusal"), "{error}");
        h.refuse.store(false, Release);
        let path = files.0.join("effect.hex");
        let original = fs::read(&path).unwrap();
        h.service.submit(Request::SaveCurrent).unwrap();
        let saved = ordinary_terminal(&mut h);
        if case == ReplayCase::RefuseAfterNewer {
            assert!(
                error.contains("recovery encountered a newer editor edit"),
                "{error}"
            );
            assert!(
                saved
                    .error
                    .unwrap()
                    .contains("editor replay recovery is unconfirmed")
            );
            assert_eq!(fs::read(path).unwrap(), original);
            return;
        }
        assert!(!error.contains("recovery"), "{error}");
        assert!(
            saved.save_complete && saved.error.is_none(),
            "{:?}",
            saved.error
        );
        let project = hex_storage::load(path).unwrap();
        let state = hex_plugin_state::decode(project.device(device).unwrap().state()).unwrap();
        assert_eq!(
            f32::from_le_bytes(state.component.unwrap()[8..12].try_into().unwrap()),
            1.5
        );
        if several_groups {
            assert_eq!(
                f64::from_le_bytes(state.component.unwrap()[32..40].try_into().unwrap()),
                1.0
            );
            let original = super::super::editor_recording::probe(&h, |reply| {
                crate::native_audio::tests::resident_preview::EditorRecordingProbe::RetiringBatch(
                    old_binding,
                    reply,
                )
            })
            .unwrap();
            assert_eq!(original.successor, successor);
            let values: Vec<_> = original
                .events()
                .iter()
                .filter(|edit| edit.kind == hex_session::service::RetiringEditorEventKind::Perform)
                .map(|edit| (edit.parameter, edit.normalized))
                .collect();
            assert_eq!(values, [(0, 0.75), (11, 0.5), (0, 0.25), (11, 0.25)]);
        }
        return;
    }
    if pre_cut || earlier_group || case == ReplayCase::MixedConflict {
        assert_eq!(
            error,
            "vst3 replacement does not already contain the retiring edit"
        );
        let mut serial = 0;
        let rows = super::super::installed_edits::query(&mut h, device, None, &mut serial)
            .rows
            .unwrap();
        let gain_id = if grouped { 0 } else { 7 };
        assert_eq!(
            rows.iter()
                .find(|row| row.id == gain_id)
                .unwrap()
                .normalized,
            if earlier_group { 0.75 } else { 0.375 }
        );
        if grouped {
            assert_eq!(
                rows.iter().find(|row| row.id == 11).unwrap().normalized,
                0.25
            );
        }
    } else {
        assert_eq!(error, "vst3 replacement accepted a newer editor edit");
    }
    let safety = h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 1 });
    assert_eq!(
        safety.device_automation_take_complete.unwrap().progress,
        hex_session::service::DeviceAutomationTakeProgress::CloseRetiringEditor(
            hex_session::service::RetiringEditorCloseBlocker::Unresolved
        )
    );

    let deadline = Instant::now() + Duration::from_secs(1);
    let needs_disposition = (
        device.value(),
        hex_ui::PluginEditorRetirementIssue::NeedsDisposition,
    );
    while !h.retirement_issues.contains(&needs_disposition) {
        h.pump();
        h.render_worker_step();
        assert!(Instant::now() < deadline, "{:?}", h.retirement_issues);
        std::thread::yield_now();
    }
    assert!(h.plugin_records.load(Acquire) >= 2);
    use crate::native_audio::tests::resident_preview::EditorRecordingProbe;
    use hex_session::service::RetiringEditorEventKind;
    let retained = super::super::editor_recording::probe(&h, |reply| {
        EditorRecordingProbe::RetiringBatch(old_binding, reply)
    })
    .unwrap();
    assert_eq!(retained.source, old_binding);
    assert_eq!(retained.successor, successor);
    if mixed || earlier_group {
        let cut = retained.predecessor_admitted_through.unwrap();
        assert_eq!(Some(cut), earlier_frontier);
        let values: Vec<_> = retained
            .events()
            .iter()
            .filter(|edit| edit.kind == RetiringEditorEventKind::Perform)
            .map(|edit| (edit.sequence <= cut, edit.parameter, edit.normalized))
            .collect();
        if mixed {
            assert_eq!(values, [(true, 0, 0.5), (false, 0, 0.75), (false, 11, 0.5)]);
        } else {
            assert_eq!(values, [(true, 0, 0.75), (true, 11, 0.5)]);
        }
        return;
    }
    assert_eq!(
        retained.predecessor_admitted_through,
        Some(if pre_cut { 14 } else { 0 })
    );
    assert_eq!(retained.events().len(), 14);
    for (index, edit) in retained.events().iter().enumerate() {
        assert_eq!(
            (edit.sequence, edit.gesture, edit.parameter),
            (index as u64 + 1, 1, 7)
        );
        match index {
            0 => assert_eq!(edit.kind, RetiringEditorEventKind::Begin),
            13 => assert_eq!(edit.kind, RetiringEditorEventKind::End),
            _ => {
                assert_eq!(edit.kind, RetiringEditorEventKind::Perform);
                assert_eq!(edit.normalized, 0.75);
            }
        }
    }

    let project = files.0.join("effect.hex");
    let media = files.0.join("mixed-rate.wav");
    let project_bytes = fs::read(&project).unwrap();
    let media_bytes = fs::read(&media).unwrap();
    let output = h.audio.as_ref().unwrap().control.output_identity();
    let requests = [
        Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        },
        Request::Open(project.clone()),
        Request::OpenWith {
            source: hex_session::service::ProjectSource::Saved(project.clone()),
            relinks: Default::default(),
        },
        Request::OpenWithAudioMode {
            source: hex_session::service::ProjectSource::Saved(project.clone()),
            relinks: Default::default(),
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        },
    ];
    for request in requests {
        h.service.submit(request).unwrap();
        let refused = ordinary_terminal(&mut h);
        assert_eq!(
            refused.error.as_deref(),
            Some("resolve the retiring vst3 editor changes before replacing the project")
        );
        assert!(refused.open_complete && !refused.project_replaced);
        assert_eq!(refused.state.project, accepted.state.project);
        assert_eq!(refused.state.revision, accepted.state.revision);
        assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    }
    let retained_after_refusal = super::super::editor_recording::probe(&h, |reply| {
        EditorRecordingProbe::RetiringBatch(old_binding, reply)
    })
    .unwrap();
    assert_eq!(retained_after_refusal.source, retained.source);
    assert_eq!(retained_after_refusal.successor, retained.successor);
    assert_eq!(retained_after_refusal.events(), retained.events());
    h.service.shutdown(Duration::from_secs(1)).unwrap();
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .shutdown_requested()
    );
    assert_eq!(hex_audio::audio_lifecycle_failure(), None);
    h.release_relinquished_output();
    wait(|| hex_audio::audio_lifecycle_failure().is_some());
    assert_eq!(
        hex_audio::audio_lifecycle_failure(),
        Some(hex_audio::AudioLifecycleFailure::RetiringEditor)
    );
    let mut blocked = ResidentHarness::with_open_gate(8192, Arc::new(AtomicBool::new(false)));
    assert!(blocked.next().error.is_none());
    wait(|| {
        blocked.pump();
        !blocked.unavailable.is_empty()
    });
    assert_eq!(
        blocked.unavailable,
        ["vst3 editor shutdown is unresolved · original source retained · audio reopen refused",]
    );
    assert!(blocked.audio.is_none());
    assert_eq!(fs::read(project).unwrap(), project_bytes);
    assert_eq!(fs::read(media).unwrap(), media_bytes);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_repeated_state_history_reclaims_superseded_instances() {
    let _native = native_fixture();
    for playing in [false, true] {
        repeated_state_history(playing);
    }
}
fn repeated_state_history(playing: bool) {
    let (mut h, files, saved) = fixture(Start::Saved);
    let device = saved.project.main_devices()[0].id();
    let replacement = with_gain(saved.project.main_devices()[0].state(), 0.75);
    let original_bytes = fs::read(files.0.join("effect.hex")).unwrap();
    let (mut applied, _) = h.applied(0);
    if playing {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        }))
        .unwrap();
    let replaced = ordinary_terminal(&mut h);
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    (applied, _) = h.applied(applied);
    // More changes than the fixed native pool can hold. Render each actual
    // finite outgoing tail and reclaim its original graph before the next edit;
    // a current/tail alias may never be treated as a reusable history instance.
    for cycle in 0..12 {
        for (request, expected) in [
            (Request::Undo, &saved.project),
            (Request::Redo, &replaced.state.project),
        ] {
            for _ in 0..8 {
                h.render_plugin_block();
                workers_idle(&h);
                h.pump();
            }
            h.service.submit(request).unwrap();
            let event = ordinary_terminal(&mut h);
            assert!(
                event.error.is_none(),
                "playing={playing}, cycle {cycle}: {:?}, retirement issues: {:?}, source: {:?}",
                event.error,
                h.retirement_issues,
                h.plugin_progress.lock().unwrap(),
            );
            assert_eq!(&event.state.project, expected);
            (applied, _) = h.applied(applied);
        }
    }
    // Graph ACK finishes the edit. The original callback return and SDK
    // seal complete separately. Keep driving both owners after the last edit.
    let deadline = Instant::now() + Duration::from_millis(500);
    while h.plugin_records.load(Acquire) > 1 {
        h.render_plugin_block();
        workers_idle(&h);
        h.pump();
        assert!(
            Instant::now() < deadline,
            "superseded sources did not retire: {:?}",
            h.retirement_issues,
        );
        std::thread::yield_now();
    }
    assert_eq!(h.plugin_records.load(Acquire), 1);
    assert!(h.retirement_issues.is_empty());
    assert_eq!(
        fs::read(files.0.join("effect.hex")).unwrap(),
        original_bytes
    );
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_state_replacement_preserves_device_and_one_history_edge() {
    let _native = native_fixture();
    let (mut h, files, saved) = fixture(Start::Saved);
    let original = &saved.project.main_devices()[0];
    let device = original.id();
    let replacement = with_gain(original.state(), 0.75);
    let bytes = fs::read(files.0.join("effect.hex")).unwrap();
    let (mut applied, _) = h.applied(0);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        }))
        .unwrap();
    let replaced = ordinary_terminal(&mut h);
    assert!(
        replaced.error.is_none(),
        "actual replacement must prepare its own matching native state: {:?}",
        replaced.error
    );
    assert_eq!(replaced.state.revision, saved.revision + 1);
    assert_eq!(replaced.state.project.main_devices()[0].id(), device);
    assert_eq!(
        replaced.state.project.main_devices()[0].state(),
        &replacement
    );
    (applied, _) = h.applied(applied);
    assert_eq!(h.plugin_records.load(Acquire), 2);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), replacement.type_id()),
        Some(true)
    );
    h.service.submit(Request::Undo).unwrap();
    let undone = ordinary_terminal(&mut h);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, saved.project);
    (applied, _) = h.applied(applied);
    h.service.submit(Request::Redo).unwrap();
    let redone = ordinary_terminal(&mut h);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, replaced.state.project);
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::Edit(Command::RenameTrack {
            track: redone.state.project.tracks()[0].id(),
            name: "renamed".into(),
        }))
        .unwrap();
    let renamed = ordinary_terminal(&mut h);
    assert!(renamed.error.is_none(), "{:?}", renamed.error);
    h.applied(applied);
    if hex_plugin_state::decode(&replacement)
        .unwrap()
        .controller
        .is_some()
    {
        let trace = std::env::var_os("HEX_ORIGINAL_RESTORE_TRACE")
            .expect("controller fixture requires its explicit trace");
        let trace = fs::read(trace).unwrap();
        let observed: Vec<_> = trace
            .chunks_exact(4)
            .map(|part| i32::from_ne_bytes(part.try_into().unwrap()))
            .collect();
        assert_eq!(observed.len(), 15);
        assert_eq!(&observed[..3], &[1, 1, 1]);
        assert_eq!(
            &observed[5..8],
            &[
                i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
                i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
                91,
            ]
        );
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::new();
    for _ in 0..6 {
        pcm.extend(h.render_plugin_block());
        workers_idle(&h);
    }
    assert!(pcm[..512].iter().all(|frame| *frame == [0.0; 2]));
    for (index, frame) in pcm.iter().enumerate().skip(512) {
        let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
        assert_eq!(
            *frame,
            [0.25 * 0.75 * scale; 2],
            "redone native state sample {index}"
        );
    }
    assert_eq!(fs::read(files.0.join("effect.hex")).unwrap(), bytes);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_reused_graph_document_cancellation_respects_admission() {
    let _native = native_fixture();
    for cancel_before_admission in [true, false] {
        let (mut h, files, before) = fixture(Start::Saved);
        let original = fs::read(files.0.join("effect.hex")).unwrap();
        h.applied(0);
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let clip = before.project.tracks()[0].clips()[0].id();
        let edit = Request::Edit(Command::DuplicateClip(clip));
        let (resume, held_work) = if cancel_before_admission {
            let (ready, resume, _) = h.probe_next_native_publication();
            h.service.submit(edit).unwrap();
            graph_publication_ready(&mut h, &ready);
            (resume, None)
        } else {
            let (release, holder) = hold_graph_publication(&mut h, edit);
            (release, Some(holder))
        };
        h.service
            .submit(Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
        resume.send(()).unwrap();
        if let Some(holder) = held_work {
            holder.join().unwrap();
        }
        let finished = h.next();
        if cancel_before_admission {
            assert!(
                finished
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("cancelled")),
                "{:?}",
                finished.error
            );
            assert_eq!(finished.state.project, before.project);
            assert_eq!(finished.state.revision, before.revision);
            assert_eq!(
                (finished.state.can_undo, finished.state.can_redo),
                (before.can_undo, before.can_redo)
            );
        } else {
            assert!(finished.error.is_none(), "{:?}", finished.error);
            assert_eq!(finished.state.revision, before.revision + 1);
            assert_eq!(finished.state.project.tracks()[0].clips().len(), 2);
            assert!(finished.state.can_undo);
        }
        let replaced = h.next();
        assert!(replaced.project_replaced && replaced.state.project.tracks().is_empty());
        let context = replaced
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context();
        let terminal = (context.project_generation, old_output);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if h.released.last() == Some(&terminal) {
                h.release_relinquished_output();
                h.wait_fresh_output(old_output);
                break;
            }
            if h.rebound == Some(terminal) {
                assert_eq!(
                    h.audio.as_ref().unwrap().control.output_identity(),
                    Some(old_output)
                );
                break;
            }
            assert!(Instant::now() < deadline, "document output did not settle");
            h.render_worker_step();
            std::thread::yield_now();
        }
        let (_, metadata) = h.applied(0);
        assert_eq!(metadata.context, context);
        assert_eq!(fs::read(files.0.join("effect.hex")).unwrap(), original);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_graph_contention_keeps_exact_insertion_admission_cutoff() {
    let _native = native_fixture();
    for cancel_before_admission in [true, false] {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        let before = h.next();
        assert!(before.error.is_none());
        let (applied, _) = h.applied(0);
        let input = request(&before.state, PluginEffectDestination::Main);
        if cancel_before_admission {
            let (ready, resume, _) = h.probe_next_native_publication();
            h.service
                .submit(Request::InsertPluginEffect(input.clone()))
                .unwrap();
            graph_publication_ready(&mut h, &ready);
            assert!(input.control.cancel(), "pre-admission cancellation");
            resume.send(()).unwrap();
        } else {
            let (release, holder) =
                hold_graph_publication(&mut h, Request::InsertPluginEffect(input.clone()));
            let cancelled = input.control.cancel();
            release.send(()).unwrap();
            holder.join().unwrap();
            assert!(!cancelled, "admission already owns this insertion");
        }
        let finished = terminal(&mut h);
        assert_eq!(finished.plugin_effect_complete.len(), 1);
        let completion = &finished.plugin_effect_complete[0];
        assert!(Arc::ptr_eq(&completion.control, &input.control));
        assert_eq!(completion.accepted, !cancel_before_admission);
        if cancel_before_admission {
            assert!(finished.error.is_some());
            assert_eq!(finished.state.project, before.state.project);
            assert_eq!(finished.state.revision, before.state.revision);
            assert!(!finished.state.can_undo);
        } else {
            assert!(finished.error.is_none(), "{:?}", finished.error);
            assert_eq!(finished.state.revision, before.state.revision + 1);
            assert_eq!(finished.state.project.main_devices().len(), 1);
            h.applied(applied);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_pending_state_edit_keeps_fifo_save_and_document_cancellation() {
    let _native = native_fixture();
    for cancel in [false, true] {
        let (mut h, files, saved) = fixture(Start::Saved);
        let original = &saved.project.main_devices()[0];
        let state = with_gain(original.state(), 0.75);
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device: original.id(),
                state: state.clone(),
            }))
            .unwrap();
        // Hold the actual new endpoint's callback ACK. No ordinary completion
        // may retire its UI receipt before the canonical replacement terminal.
        wait(|| {
            h.pump();
            h.plugin_queued.load(Acquire)
        });
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let path = files.0.join("queued-save.hex");
        h.service.submit(Request::SaveNew(path.clone())).unwrap();
        if cancel {
            h.service
                .submit(Request::NewWithAudioMode {
                    mode: DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap();
            // New is behind Save in the unchanged FIFO, but its accepted
            // cancellation signal must release the held earlier operation.
            let refused = h.next();
            assert!(refused.request_complete && refused.error.is_some());
            assert_eq!(refused.state.project, saved.project);
            assert_eq!(refused.state.revision, saved.revision);
        } else {
            let accepted = ordinary_terminal(&mut h);
            assert!(accepted.error.is_none(), "{:?}", accepted.error);
            assert!(!accepted.save_complete);
            assert_eq!(accepted.state.project.main_devices()[0].state(), &state);
        }
        let saved_event = ordinary_terminal(&mut h);
        assert!(
            saved_event.save_complete && saved_event.error.is_none(),
            "{:?}",
            saved_event.error
        );
        let loaded = hex_storage::load(&path).unwrap();
        assert_eq!(
            loaded.main_devices()[0].state(),
            if cancel { original.state() } else { &state }
        );
        if cancel {
            let empty = h.next();
            assert!(empty.project_replaced && empty.error.is_none());
            h.release_relinquished_output();
            h.wait_fresh_output(old_output);
            h.applied(0);
            assert_eq!(h.plugin_records.load(Acquire), 0);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_contended_admission_retries_or_cancels_the_exact_insertion() {
    let _native = native_fixture();
    for cancel in [false, true] {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        let initial = h.next();
        assert!(initial.error.is_none());
        h.applied(0);
        let input = request(&initial.state, PluginEffectDestination::Main);
        let (held, observed_hold) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let holder = h.hold_native_work_on_control(held, released);
        observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        let pending = h.next();
        let cancelled = if cancel && pending.error.is_none() {
            assert!(
                input.control.cancel(),
                "unadmitted request must remain cancellable"
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let event = h.next();
                if !event.plugin_effect_complete.is_empty() {
                    break Some(event);
                }
                assert!(event.error.is_none(), "{:?}", event.error);
                assert_eq!(event.state.project, initial.state.project);
                assert!(
                    Instant::now() < deadline,
                    "contended insertion cancellation"
                );
            }
        } else {
            None
        };
        // Release actual Work even when inspecting a pre-fix refusal. No native
        // facts/endpoint/ACK are synthesized by this existing exclusion fixture.
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, initial.state.project);
        assert_eq!(pending.state.revision, initial.state.revision);
        let finished = cancelled.unwrap_or_else(|| terminal(&mut h));
        assert_eq!(finished.plugin_effect_complete.len(), 1);
        assert!(Arc::ptr_eq(
            &finished.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(finished.plugin_effect_complete[0].accepted, !cancel);
        if cancel {
            // Session may retire the explicitly cancelled pending control in
            // its normal receive, without manufacturing an error message.
            assert!(input.control.is_cancelled());
            assert!(
                finished
                    .error
                    .as_ref()
                    .is_none_or(|error| error.contains("cancelled")),
                "{:?}",
                finished.error
            );
            assert_eq!(finished.state.project, initial.state.project);
            assert_eq!(finished.state.revision, initial.state.revision);
            assert!(!finished.state.can_undo);
            assert_eq!(h.plugin_records.load(Acquire), 0);
        } else {
            assert!(finished.error.is_none(), "{:?}", finished.error);
            assert_eq!(finished.state.revision, initial.state.revision + 1);
            assert_eq!(finished.state.project.main_devices().len(), 1);
            assert_eq!(h.plugin_records.load(Acquire), 1);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_failed_state_restore_preserves_last_confirmed_state_and_history() {
    let _native = native_fixture();
    let (mut h, files, saved) = fixture(Start::Saved);
    let original = &saved.project.main_devices()[0];
    let decoded = hex_plugin_state::decode(original.state()).unwrap();
    let mut component = decoded.component.unwrap().to_vec();
    component[0] = b'!'; // Valid host envelope; actual original provider refuses its state.
    let refused = hex_plugin_state::encode(hex_plugin_state::State {
        component: Some(&component),
        ..decoded
    })
    .unwrap();
    let before = fs::read(files.0.join("effect.hex")).unwrap();
    let (applied, _) = h.applied(0);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device: original.id(),
            state: refused,
        }))
        .unwrap();
    let terminal = ordinary_terminal(&mut h);
    assert!(terminal.error.is_some());
    assert_eq!(terminal.state.project, saved.project);
    assert_eq!(terminal.state.revision, saved.revision);
    assert_eq!(
        (terminal.state.can_undo, terminal.state.can_redo),
        (saved.can_undo, saved.can_redo)
    );
    wait(|| {
        h.pump();
        h.plugin_records.load(Acquire) == 1
    });
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(fs::read(files.0.join("effect.hex")).unwrap(), before);
}

#[test]
#[ignore = "explicit original SDK helper and source receipts required"]
fn native_effect_track_group_duplicate_has_independent_instances_and_one_undo_redo() {
    let _native = native_fixture();
    let files = Files::new();
    let wave = files.wav(48_000, 2048);
    let bytes = fs::read(&wave).unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let mut state = h.next().state;
    let (mut applied, _) = h.applied(0);
    for gain in [0.75_f32, 1.25] {
        let imported = h.run(Request::Import(wave.clone()));
        (applied, _) = h.applied(applied);
        let track = imported.state.project.tracks().last().unwrap().id();
        h.service
            .submit(Request::InsertPluginEffect(request(
                &imported.state,
                PluginEffectDestination::Track(track),
            )))
            .unwrap();
        let queued = h.next();
        assert!(queued.error.is_none(), "insert queue: {:?}", queued.error);
        let inserted = terminal(&mut h);
        assert!(inserted.error.is_none(), "{:?}", inserted.error);
        state = inserted.state;
        (applied, _) = h.applied(applied);
        let device = state.project.track(track).unwrap().devices()[0].id();
        let replacement = with_gain(state.project.device(device).unwrap().state(), gain);
        h.service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device,
                state: replacement,
            }))
            .unwrap();
        let changed = ordinary_terminal(&mut h);
        assert!(changed.error.is_none(), "{:?}", changed.error);
        state = changed.state;
        (applied, _) = h.applied(applied);
    }
    let source_tracks = state.project.tracks().to_vec();
    let selected = hex_project::TrackSelectionIds::new(
        &source_tracks
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    h.service
        .submit(Request::TrackClipboard {
            project_generation: 0,
            revision: state.revision,
            action: hex_session::service::TrackClipboardAction::Duplicate {
                tracks: selected,
                without_clips: false,
            },
        })
        .unwrap();
    let duplicated = ordinary_terminal(&mut h);
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    assert_eq!(duplicated.state.revision, state.revision + 1);
    assert_eq!(duplicated.state.project.tracks().len(), 4);
    (applied, _) = h.applied(applied);
    assert_eq!(h.plugin_records.load(Acquire), 4);
    h.service.submit(Request::Undo).unwrap();
    let undone = ordinary_terminal(&mut h);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.tracks(), source_tracks);
    (applied, _) = h.applied(applied);
    let (release, holder) = hold_graph_publication(&mut h, Request::Redo);
    release.send(()).unwrap();
    holder.join().unwrap();
    let redone = ordinary_terminal(&mut h);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(
        redone.state.project.tracks(),
        duplicated.state.project.tracks()
    );
    (applied, _) = h.applied(applied);
    let device = redone.state.project.tracks()[3].devices()[0].id();
    let state = with_gain(redone.state.project.device(device).unwrap().state(), 0.75);
    h.service
        .submit(Request::Edit(Command::ReplaceDeviceState { device, state }))
        .unwrap();
    let changed = ordinary_terminal(&mut h);
    assert!(changed.error.is_none(), "{:?}", changed.error);
    h.applied(applied);
    for (original, copied) in source_tracks
        .iter()
        .zip(&changed.state.project.tracks()[2..])
    {
        assert_ne!(original.id(), copied.id());
        assert_ne!(original.devices()[0].id(), copied.devices()[0].id());
        assert_eq!(original.clips()[0].asset(), copied.clips()[0].asset());
        assert_eq!(original.clips()[0].start(), copied.clips()[0].start());
        assert_eq!(original.clips()[0].duration(), copied.clips()[0].duration());
    }
    assert_eq!(&changed.state.project.tracks()[..2], source_tracks);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::new();
    for _ in 0..6 {
        pcm.extend(h.render_plugin_block());
        workers_idle(&h);
    }
    assert!(pcm[..512].iter().all(|frame| *frame == [0.0; 2]));
    for (index, frame) in pcm.iter().enumerate().skip(512) {
        let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
        assert_eq!(
            *frame,
            [0.25 * 3.5 * scale; 2],
            "independent copy sample {index}"
        );
    }
    assert_eq!(fs::read(wave).unwrap(), bytes);
}
