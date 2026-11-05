//! Saved opaque state through the actual canonical worker and native output.
//! An uncatalogued source must remain explicitly unavailable, never become a
//! fresh-default instance or prevent recovery of the saved project bytes.
use super::*;
use hex_session::service::NativeDeviceState;
mod edits;
mod save;

/// Tests that hold a fresh callback before its first graph use the real
/// playing-output fallback, independently of the retained-output regressions.
fn play_outgoing_before_open(h: &mut ResidentHarness) {
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
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_captures_processed_state_without_mutating_history() {
    let _native = native_fixture();
    let (mut h, files, before) = fixture(Start::Saved);
    let path = files.0.join("effect.hex");
    let original = fs::read(&path).unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    let saved = h.run(Request::SaveCurrent);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert!(saved.save_complete);
    assert_eq!(saved.state.project, before.project);
    assert_eq!(saved.state.revision, before.revision);
    assert_eq!(saved.state.can_undo, before.can_undo);
    assert_eq!(saved.state.can_redo, before.can_redo);
    assert_ne!(fs::read(&path).unwrap(), original);
    let loaded = hex_storage::load(&path).unwrap();
    let current = hex_plugin_state::decode(loaded.main_devices()[0].state()).unwrap();
    let previous = hex_plugin_state::decode(before.project.main_devices()[0].state()).unwrap();
    assert_eq!(current.identity, previous.identity);
    assert_ne!(current.component, previous.component);
    assert_eq!(loaded.tracks(), before.project.tracks());
}

struct SavedEffectFixture {
    files: Files,
    path: PathBuf,
    catalog: PathBuf,
    project: hex_project::Project,
    expected: NativeDeviceState,
    old_output: u64,
}
fn nondefault_fixture() -> SavedEffectFixture {
    let (old, files, saved) = fixture(Start::Saved);
    let device = &saved.project.main_devices()[0];
    let captured = hex_plugin_state::decode(device.state()).unwrap();
    let mut component = captured.component.unwrap().to_vec();
    assert_eq!(&component[..4], b"HXPS");
    component[4..8].copy_from_slice(&0_u32.to_le_bytes());
    component[8..12].copy_from_slice(&0.75_f32.to_le_bytes());
    let mut controller = captured.controller.map(<[u8]>::to_vec);
    if let Some(controller) = controller.as_mut() {
        assert_eq!(&controller[..4], b"HXCT");
        controller[4..8].copy_from_slice(&91_u32.to_le_bytes());
    }
    let expected = hex_plugin_state::encode(hex_plugin_state::State {
        identity: captured.identity,
        component: Some(&component),
        controller: controller.as_deref(),
    })
    .unwrap();
    let mut editor = hex_project::Editor::new(saved.project.as_ref().clone(), 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device.id(),
            state: expected.clone(),
        })
        .unwrap();
    let path = files.0.join("nondefault.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let expected_project = editor.project().clone();
    let old_output = old
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    drop(old);

    let mut selected = selection();
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Fx", 0).unwrap();
    let catalog = files.0.canonicalize().unwrap().join("plugin-catalog.json");
    SavedEffectFixture {
        files,
        path,
        catalog,
        project: expected_project,
        expected,
        old_output,
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn retained_output_restores_saved_vst3_before_retiring_the_previous_generation() {
    let _native = native_fixture();
    let saved = nondefault_fixture();
    let device = saved.project.main_devices()[0].id();
    let original = fs::read(&saved.path).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    let (mut revision, _) = h.applied(0);
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let mut previous = None;
    for _ in 0..2 {
        let opened = h.run(Request::Open(saved.path.clone()));
        assert_eq!(opened.state.project.as_ref(), &saved.project);
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire),
            revision,
            "a saved replacement cannot apply before the callback's stopped witness"
        );
        let applied = super::super::resident_lifecycle::finish_replacement(&mut h, &opened, output);
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            Some(output)
        );
        assert!(applied > revision);
        revision = applied;
        assert_eq!(
            h.submitted_observation()
                .unwrap()
                .foreign_processor(device.value(), saved.expected.type_id()),
            Some(true),
            "the first applied successor already owns the actual restored source"
        );
        let restored = h.next();
        assert!(restored.error.is_none() && !restored.request_complete);
        assert_eq!(restored.state.project, opened.state.project);
        wait(|| {
            h.render_plugin_block();
            let progress = *h.plugin_progress.lock().unwrap();
            h.plugin_records.load(Acquire) == 1
                && progress.accepted
                && progress
                    .binding
                    .is_some_and(|binding| Some(binding) != previous)
        });
        previous = h.plugin_progress.lock().unwrap().binding;
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
        assert_eq!(pcm, [[if block == 2 { 0.09375 } else { 0.0 }; 2]; 256]);
    }
    assert_eq!(fs::read(saved.path).unwrap(), original);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts; variant37 additionally exercises controller state"]
fn native_effect_saved_state_restores_fresh_instance_and_nondefault_pcm() {
    let _native = native_fixture();
    let SavedEffectFixture {
        files,
        path,
        catalog,
        project: expected_project,
        expected,
        old_output,
    } = nondefault_fixture();
    let device_id = expected_project.main_devices()[0].id();
    let controller = hex_plugin_state::decode(&expected)
        .unwrap()
        .controller
        .is_some();
    let original_saved = fs::read(&path).unwrap();
    let original_wave = fs::read(files.0.join("mixed-rate.wav")).unwrap();
    let catalog_bytes = fs::read(&catalog).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        catalog.clone(),
    );
    let initial = h.next();
    assert!(initial.error.is_none(), "{:?}", initial.error);
    h.applied(0);
    let initial_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    assert_ne!(initial_output, old_output);
    play_outgoing_before_open(&mut h);
    let opened = h.run(Request::Open(path.clone()));
    assert!(
        opened.error.is_none() && opened.project_replaced,
        "{:?}",
        opened.error
    );
    assert_eq!(opened.state.project.as_ref(), &expected_project);
    assert!(!opened.state.can_undo && !opened.state.can_redo);
    h.release_relinquished_output();
    h.wait_fresh_output(initial_output);
    // Hold the real callback before endpoint installation. The Source's
    // actual queued endpoint is not an ACK or permission to publish dry audio.
    wait(|| {
        h.pump();
        h.plugin_queued.load(Acquire)
    });
    for _ in 0..3 {
        h.pump();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire),
            0
        );
        assert!(h.submitted.is_none());
    }
    h.service.submit(Request::RestoreStoppedPlayback).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|error| error.contains("vst3 state restoration owns publication")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.project, opened.state.project);
    // The first graph applied to this fresh output must already contain the
    // restored source. Installing a dry placeholder first fails this oracle.
    let (_, metadata) = h.applied(0);
    assert_eq!(
        metadata.context,
        opened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
    );
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device_id.value(), expected.type_id()),
        Some(true)
    );
    assert_eq!(h.plugin_records.load(Acquire), 1);
    if controller {
        let trace = std::env::var_os("HEX_ORIGINAL_RESTORE_TRACE")
            .expect("controller case requires the original fixture's explicit restore trace");
        let bytes = fs::read(trace).unwrap();
        let observed: Vec<_> = bytes
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
                91
            ]
        );
    }
    let restored = h.next();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert!(!restored.request_complete);
    assert_eq!(restored.state.revision, opened.state.revision);
    assert_eq!(restored.state.project, opened.state.project);
    assert_eq!(restored.state.project.main_devices()[0].state(), &expected);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut pcm = Vec::new();
    for block in 0..6 {
        pcm.extend(h.render_plugin_block());
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    assert!(pcm[..512].iter().all(|frame| *frame == [0.0; 2]));
    for (index, frame) in pcm.iter().enumerate().skip(512) {
        let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
        assert_eq!(
            *frame,
            [0.25 * 0.75 * scale; 2],
            "restored source sample {index}"
        );
    }
    assert_eq!(fs::read(path).unwrap(), original_saved);
    assert_eq!(
        fs::read(files.0.join("mixed-rate.wav")).unwrap(),
        original_wave
    );
    assert_eq!(fs::read(catalog).unwrap(), catalog_bytes);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_reopen_source_refusals_preserve_state_without_defaults() {
    let _native = native_fixture();
    for failure in 0..5 {
        let mut saved = nondefault_fixture();
        let module = saved.files.0.canonicalize().unwrap().join("original.vst3");
        match failure {
            0 => fs::remove_dir_all(&module).unwrap(),
            1 | 2 => {
                let target = module.join(if failure == 1 {
                    "Contents/MacOS/fixture"
                } else {
                    "Contents/Info.plist"
                });
                let mut bytes = fs::read(&target).unwrap();
                bytes.push(0);
                fs::write(target, bytes).unwrap();
            }
            3 => {
                let copy = saved.files.0.canonicalize().unwrap().join("second.vst3");
                ui::copy_original_module(&module, &copy);
                let mut selected = selection();
                selected.module = copy;
                hex_ui::write_plugin_effect_test_catalog(&saved.files.0, &selected, "Fx", 0)
                    .unwrap();
            }
            _ => {
                // Valid generic envelope, deliberately refused opaque vendor
                // state. Only the actual plugin interprets these chunk bytes.
                let decoded = hex_plugin_state::decode(&saved.expected).unwrap();
                let mut component = decoded.component.unwrap().to_vec();
                component[0] = b'!';
                saved.expected = hex_plugin_state::encode(hex_plugin_state::State {
                    identity: decoded.identity,
                    component: Some(&component),
                    controller: decoded.controller,
                })
                .unwrap();
                let mut editor = hex_project::Editor::new(saved.project.clone(), 16).unwrap();
                editor
                    .execute(Command::ReplaceDeviceState {
                        device: saved.project.main_devices()[0].id(),
                        state: saved.expected.clone(),
                    })
                    .unwrap();
                saved.project = editor.project().clone();
                saved.path = saved.files.0.join("refused.hex");
                hex_storage::save_new(&saved.path, &saved.project).unwrap();
            }
        }
        let before = fs::read(&saved.path).unwrap();
        let catalog_before = fs::read(&saved.catalog).unwrap();
        let mut h = ResidentHarness::with_plugins_and_catalog(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            saved.catalog.clone(),
        );
        let initial = h.next();
        assert!(initial.error.is_none(), "{:?}", initial.error);
        h.applied(0);
        let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let opened = h.run(Request::Open(saved.path.clone()));
        assert_eq!(opened.state.project.as_ref(), &saved.project);
        super::super::resident_lifecycle::finish_replacement(&mut h, &opened, old);
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            Some(old)
        );
        let device = &saved.project.main_devices()[0];
        assert_eq!(
            h.submitted_observation()
                .unwrap()
                .foreign_processor(device.id().value(), saved.expected.type_id()),
            Some(false),
            "failure{failure}"
        );
        assert_eq!(h.plugin_records.load(Acquire), 0);
        assert_eq!(
            h.plugin_progress.lock().unwrap().restore_refused,
            failure == 4,
            "failure{failure}: only actual native restore refusal must report a failed preparation"
        );
        assert_eq!(
            opened.state.project.main_devices()[0].state(),
            &saved.expected
        );
        assert!(!opened.state.can_undo && !opened.state.can_redo);
        assert_eq!(fs::read(&saved.path).unwrap(), before);
        assert_eq!(fs::read(&saved.catalog).unwrap(), catalog_before);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        assert_eq!(
            h.render(),
            [[0.25; 2]; 64],
            "failure{failure}: unrelated audio must remain playable"
        );
    }
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_reopen_preserves_sealed_same_audio_authoring_alias() {
    let _native = native_fixture();
    let saved = nondefault_fixture();
    let mut editor = hex_project::Editor::new(saved.project.clone(), 16).unwrap();
    let class = hex_engine::NativeDeviceClass::Effect;
    let utility_type = hex_engine::native_device_catalog_for_class(class)
        .unwrap()
        .into_iter()
        .find(|entry| entry.type_id.as_str() == Ok("hex.utility"))
        .unwrap()
        .type_id;
    let Created::Device(utility) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state_for_class(class, utility_type).unwrap(),
        })
        .unwrap()
    else {
        panic!("utility")
    };
    let path = saved.files.0.join("same-audio.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    play_outgoing_before_open(&mut h);
    let opened = h.run(Request::Open(path.clone()));
    let source = opened
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    h.release_relinquished_output();
    h.wait_fresh_output(old);
    wait(|| {
        h.pump();
        h.plugin_queued.load(Acquire)
    });
    let compared = h.run(Request::CompareDevice {
        owner: hex_session::service::preset::Owner {
            project_generation: source.project_generation,
            revision: opened.state.revision,
            device: utility,
            class,
            type_id: utility_type,
        },
        expected: hex_project::DeviceVariant::A,
        action: hex_project::DeviceComparisonAction::Switch,
    });
    assert_eq!(compared.state.revision, opened.state.revision + 1);
    assert_eq!(
        compared.state.audio_authoring_revision(source),
        Some(compared.state.revision)
    );
    h.applied(0);
    let restored = h.next();
    assert!(
        restored.error.is_none(),
        "sealed same-audio alias must retain the matching restoration: {:?}",
        restored.error
    );
    assert_eq!(restored.state.revision, compared.state.revision);
    assert_eq!(restored.state.project, compared.state.project);
    assert_eq!(
        h.submitted_observation().unwrap().foreign_processor(
            saved.project.main_devices()[0].id().value(),
            saved.expected.type_id()
        ),
        Some(true)
    );
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let undone = h.run(Request::Undo);
    assert_eq!(
        undone.state.project, opened.state.project,
        "restore must add no history edit after the metadata switch"
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_reopen_document_replacement_retires_exact_unacknowledged_source() {
    let _native = native_fixture();
    let saved = nondefault_fixture();
    let before = fs::read(&saved.path).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    play_outgoing_before_open(&mut h);
    let opened = h.run(Request::Open(saved.path.clone()));
    h.release_relinquished_output();
    h.wait_fresh_output(old);
    wait(|| {
        h.pump();
        h.plugin_queued.load(Acquire)
    });
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        0
    );
    let unacknowledged = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let empty = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(empty.project_replaced && empty.state.revision == opened.state.revision + 1);
    h.release_relinquished_output();
    h.wait_fresh_output(unacknowledged);
    h.applied(0);
    assert!(empty.state.project.main_devices().is_empty());
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert_eq!(fs::read(saved.path).unwrap(), before);
}

#[gpui::test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_reopen_paints_pending_until_real_ack_then_restored_canonical(
    cx: &mut gpui::TestAppContext,
) {
    let _native = native_fixture();
    let saved = nondefault_fixture();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    play_outgoing_before_open(&mut h);
    let callback = Arc::clone(&h.callback);
    let queued = Arc::clone(&h.plugin_queued);
    h.service.submit(Request::Open(saved.path.clone())).unwrap();
    let window = hex_ui::open_sampler_load_test_backend(
        cx,
        h.into_plugin_ui_backend(),
        saved.files.0.clone(),
        saved.files.0.clone(),
    )
    .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window, cx);
    let mut old_closed = false;
    let mut step =
        |cx: &mut gpui::TestAppContext, visual: &mut gpui::VisualTestContext, release: bool| {
            let mut slot = callback.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(HeadlessBackend::shutdown_requested)
            {
                slot.take().unwrap().close();
                old_closed = true;
            } else if (release || !old_closed)
                && let Some(backend) = slot.as_mut()
            {
                backend
                    .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                    .unwrap();
            }
            drop(slot);
            ui::repaint(cx, visual);
        };
    ui::until(
        "fresh source reached its actual queued endpoint without callback ACK",
        || {
            step(cx, &mut visual, false);
            queued.load(Acquire)
                && cx
                    .update(|app| hex_ui::plugin_effect_test_view(window, app))
                    .unwrap()
                    .state
                    .as_ref()
                    .is_some_and(|state| state.project.as_ref() == &saved.project)
        },
    );
    ui::click(&mut visual, "main-track-header");
    let device = saved.project.main_devices()[0].id().value();
    let pending = Box::leak(format!("device-vst3-pending-{device}").into_boxed_str());
    let active = Box::leak(format!("device-vst3-active-{device}").into_boxed_str());
    let unavailable = Box::leak(format!("device-vst3-unavailable-{device}").into_boxed_str());
    for _ in 0..3 {
        step(cx, &mut visual, false);
        assert!(visual.debug_bounds(pending).is_some());
        assert!(
            visual.debug_bounds(active).is_none() && visual.debug_bounds(unavailable).is_none()
        );
        let view = cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap();
        assert!(
            view.applied.is_none() && !view.pending,
            "reopen is not a new insertion request"
        );
        assert_eq!(view.state.unwrap().project.as_ref(), &saved.project);
    }
    ui::until(
        "matching restored graph actually applied and painted",
        || {
            step(cx, &mut visual, true);
            let view = cx
                .update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap();
            assert!(view.error.is_none(), "{:?}", view.error);
            view.applied.is_some() && visual.debug_bounds(active).is_some()
        },
    );
    for _ in 0..3 {
        step(cx, &mut visual, true);
        assert!(visual.debug_bounds(active).is_some());
        assert!(
            visual.debug_bounds(pending).is_none() && visual.debug_bounds(unavailable).is_none()
        );
        let state = cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
            .state
            .unwrap();
        assert_eq!(state.project.as_ref(), &saved.project);
        assert_eq!(state.project.main_devices()[0].state(), &saved.expected);
        assert!(!state.can_undo && !state.can_redo);
    }
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_saved_project_without_resolved_source_retains_opaque_state() {
    let _native = native_fixture();
    for start in [Start::Reopened, Start::EmptyThenReopened] {
        let (h, files, state) = fixture(start);
        drop(h);
        // Opening recoverable state must not require an available output or
        // helper. Offline custody remains distinct from callback application.
        let mut offline = ResidentHarness::with_open_gate(8192, Arc::new(AtomicBool::new(false)));
        assert!(offline.next().error.is_some());
        let opened = offline.run(Request::Open(files.0.join("effect.hex")));
        assert!(opened.error.is_none(), "{:?}", opened.error);
        assert!(opened.open_complete && opened.project_replaced);
        assert_eq!(opened.state.project, state.project);
        assert!(offline.audio.is_none() && offline.submitted.is_none());
        assert_eq!(offline.plugin_records.load(Acquire), 0);
    }
}

enum Start {
    Unsaved,
    Saved,
    Reopened,
    EmptyThenReopened,
}

fn fixture(start: Start) -> (ResidentHarness, Files, Arc<hex_session::service::ViewState>) {
    let files = Files::new();
    let wave = files.wav(48_000, 2048);
    let wave_bytes = fs::read(&wave).unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(wave.clone()));
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    h.applied(applied);
    if matches!(start, Start::Unsaved) {
        return (h, files, accepted.state);
    }
    let expected = accepted.state.project.main_devices()[0].state().clone();
    let path = files.0.join("effect.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete);
    let saved_bytes = fs::read(&path).unwrap();
    let stored = hex_storage::load(&path).unwrap();
    assert_eq!(stored.main_devices()[0].state(), &expected);

    if matches!(start, Start::Saved) {
        return (h, files, saved.state);
    }

    if matches!(start, Start::EmptyThenReopened) {
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let empty = h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        });
        assert!(empty.project_replaced);
        super::super::resident_lifecycle::finish_replacement(&mut h, &empty, old_output);
        wait(|| {
            h.render();
            h.plugin_records.load(Acquire) == 0
        });
    }
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();

    // No catalog/resolver grants a source for this new document. The file's
    // valid opaque envelope must still survive as an unavailable device.
    h.service.submit(Request::Open(path.clone())).unwrap();
    let opened = h.next();
    assert!(
        opened.error.is_none(),
        "saved opaque vst3 state must reopen without defaults or data loss: {:?}",
        opened.error
    );
    assert!(opened.project_replaced && opened.open_complete);
    assert_eq!(opened.state.project.main_devices()[0].state(), &expected);
    assert!(!opened.state.can_undo && !opened.state.can_redo);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    let (_, metadata) = h.applied(0);
    assert_eq!(
        metadata.context,
        opened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
    );
    let device = &opened.state.project.main_devices()[0];
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.id().value(), device.state().type_id()),
        Some(false)
    );
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert_eq!(fs::read(path).unwrap(), saved_bytes);
    assert_eq!(fs::read(wave).unwrap(), wave_bytes);
    (h, files, opened.state)
}

#[gpui::test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_reopened_unavailable_status_follows_actual_applied_graph(
    cx: &mut gpui::TestAppContext,
) {
    let _native = native_fixture();
    let (h, files, state) = fixture(Start::Saved);
    let device = state.project.main_devices()[0].id();
    // Real read-only request delivers the current canonical snapshot to the
    // newly attached view; actual output/submission events carry availability.
    let clip = state.project.tracks()[0].clips()[0].id();
    h.service.submit(Request::CopyClips(vec![clip])).unwrap();
    let callback = Arc::clone(&h.callback);
    h.service
        .submit(Request::Open(files.0.join("effect.hex")))
        .unwrap();
    let backend = h.into_plugin_ui_backend();
    let window =
        hex_ui::open_sampler_load_test_backend(cx, backend, files.0.clone(), files.0.clone())
            .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window, cx);
    let step = |cx: &mut gpui::TestAppContext, visual: &mut gpui::VisualTestContext| {
        let mut slot = callback.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(HeadlessBackend::shutdown_requested)
        {
            slot.take().unwrap().close();
        } else if let Some(backend) = slot.as_mut() {
            backend
                .render_native_direct(&mut [[0.0; 2]; 64], &mut [0.0; 128])
                .unwrap();
        }
        drop(slot);
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
    };
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        step(cx, &mut visual);
        if cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
            .state
            .as_ref()
            .is_some_and(|s| {
                s.canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation
                    == 1
            })
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "actual reopened canonical state did not arrive"
        );
        std::thread::yield_now();
    }
    let header = visual.debug_bounds("main-track-header").unwrap();
    visual.simulate_click(
        gpui::point(header.origin.x + gpui::px(8.0), header.center().y),
        gpui::Modifiers::default(),
    );
    let unavailable =
        Box::leak(format!("device-vst3-unavailable-{}", device.value()).into_boxed_str());
    while visual.debug_bounds(unavailable).is_none() {
        step(cx, &mut visual);
        let view = cx
            .update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap();
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(
            Instant::now() < end,
            "unavailable status did not follow actual reopened graph: {}",
            view.status
        );
        std::thread::yield_now();
    }
    for _ in 0..3 {
        step(cx, &mut visual);
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-vst3-unavailable-{}", device.value()).into_boxed_str()
                ))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-vst3-active-{}", device.value()).into_boxed_str()
                ))
                .is_none()
        );
        assert_eq!(
            cx.update(|app| hex_ui::plugin_effect_test_view(window, app))
                .unwrap()
                .state
                .unwrap()
                .project,
            state.project
        );
    }
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}
