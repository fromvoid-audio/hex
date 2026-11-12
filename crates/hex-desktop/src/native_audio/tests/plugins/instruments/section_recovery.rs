//! Actual native source refusal through the rendered editor and Session FIFO.
use super::*;
use gpui::{Modifiers, MouseButton, VisualTestContext};

#[gpui::test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn in_flight_duplicate_recovers_a_revoked_native_source(cx: &mut gpui::TestAppContext) {
    const CHILD: &str = "HEX_DESKTOP_IN_FLIGHT_NATIVE_RECOVERY";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
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
    let files = Files::new();
    let mut selected = selection();
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    super::super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Fx", 0).unwrap();
    let catalog = files.0.canonicalize().unwrap().join("plugin-catalog.json");
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        48_000,
        catalog,
        2_000_000,
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 192_000)));
    (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.track(track).unwrap().clips()[0].id();
    let mut insertion = request(&imported.state, PluginEffectDestination::Main);
    insertion.selection = selected;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    h.applied(applied);
    let original = inserted.state.project.snapshot();
    let revision = inserted.state.revision;
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    render_completed_block(&h);
    let (entered, resume, _busy) = h.probe_next_native_publication();
    h.service.submit(Request::CopyClips(vec![clip])).unwrap();
    let callback = Arc::clone(&h.callback);
    let backend = h.into_plugin_ui_backend();
    let window =
        hex_ui::open_sampler_load_test_backend(cx, backend, files.0.clone(), files.0.clone())
            .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, _| window.activate_window());
    let view = |cx: &mut gpui::TestAppContext| {
        cx.update(|app| hex_ui::plugin_effect_test_view(window, app))
            .unwrap()
    };
    super::super::ui::until("initial native project", || {
        super::super::ui::repaint(cx, &mut visual);
        view(cx)
            .state
            .as_ref()
            .is_some_and(|state| state.revision == revision)
    });
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    let start = gpui::point(lane.origin.x + lane.size.width * 0.05, lane.center().y);
    let end = gpui::point(lane.origin.x + lane.size.width * 0.15, start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-d");
    super::super::ui::until("copy reaches native publication", || {
        super::super::ui::repaint(cx, &mut visual);
        entered.try_recv().is_ok()
    });
    {
        let mut slot = callback.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        for _ in 0..64 {
            backend
                .render_native_direct(&mut [[0.0; 2]; 256], &mut [0.0; 512])
                .unwrap();
        }
    }
    let failed_cursor = view(cx).transport.unwrap().0;
    resume.send(()).unwrap();
    let mut saw_recovery = false;
    let step = |cx: &mut gpui::TestAppContext, visual: &mut VisualTestContext| {
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
        super::super::ui::repaint(cx, visual);
    };
    super::super::ui::until("copy recovers and commits", || {
        step(cx, &mut visual);
        let snapshot = view(cx);
        saw_recovery |= snapshot.status.contains("restarting audio");
        snapshot
            .state
            .as_ref()
            .is_some_and(|state| state.revision == revision + 1)
            && snapshot.applied.is_some_and(|source| {
                source.output_identity != old_output
                    && source.context.editor_revision == revision + 1
            })
            && snapshot
                .transport
                .is_some_and(|(position, playing)| playing && position >= failed_cursor)
    });
    assert!(
        saw_recovery,
        "real source refusal must initiate audio recovery"
    );
    let copied = view(cx).state.unwrap();
    assert!(copied.project.track(track).unwrap().clips().len() > 1);
    assert!(copied.can_undo);
    visual.simulate_keystrokes("cmd-z");
    super::super::ui::until("undo restores the source project", || {
        step(cx, &mut visual);
        view(cx)
            .state
            .as_ref()
            .is_some_and(|state| state.revision == revision + 2)
    });
    let mut restored = view(cx).state.unwrap().project.snapshot();
    restored.next_id = original.next_id;
    assert_eq!(restored, original);
}
