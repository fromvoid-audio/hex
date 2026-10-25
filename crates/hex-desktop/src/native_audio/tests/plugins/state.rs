//! Saved opaque state through the actual canonical worker and native output.
//! An uncatalogued source must remain explicitly unavailable, never become a
//! fresh-default instance or prevent recovery of the saved project bytes.
use super::*;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_saved_project_without_resolved_source_retains_opaque_state() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
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
        h.release_relinquished_output();
        h.wait_fresh_output(old_output);
        h.applied(0);
        assert_eq!(h.plugin_records.load(Acquire), 0);
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
    let _native = NATIVE_FIXTURE.lock().unwrap();
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
