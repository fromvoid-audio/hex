use super::*;

pub(super) struct BrowserRoot(std::path::PathBuf);
impl Drop for BrowserRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
// Match the existing exclusive output lease for these private headless fixtures.
// No global environment/settings mutation, backend redesign or lease bypass.
/// The process-wide headless output's single test serializer. A second,
/// independent mutex let its holders open or retire the output concurrently
/// with `serial_output` holders, stalling retirement (Pulse #3857).
pub(in crate::editor) fn status_fixture_guard() -> std::sync::MutexGuard<'static, ()> {
    hex_audio::worker_fixture::serial_output()
}
pub(in crate::editor) fn headless_status_output() -> (
    hex_audio::WorkerPreparedOutput,
    hex_audio::worker_fixture::HeadlessBackend,
) {
    // Drop retires the previous headless carriers on the existing lifecycle
    // worker. Wait only for that bounded teardown, never open an OS backend.
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => return output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => {
                panic!("headless status fixture could not acquire its exclusive lease: {error}")
            }
        }
    }
}
pub(super) fn bounded_window(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
    track: TrackId,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>, BrowserRoot) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // A reused PID can meet a leftover directory; take the next free name.
    let root = loop {
        let path = std::env::temp_dir().join(format!(
            "hex-eq-layout-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => break BrowserRoot(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create eq layout root: {error}"),
        }
    };
    std::fs::create_dir(root.0.join("samples")).unwrap();
    #[cfg(target_os = "macos")]
    cx.use_native_text_system()
        .expect("opt into native bundled-font shaping before windows");
    let bootstrap = cx.add_window(|_, _| gpui::Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            if let Err(error) = theme::load_fonts(cx) {
                #[cfg(target_os = "macos")]
                panic!("native bundled font: {error}");
                #[cfg(not(target_os = "macos"))]
                eprintln!("UNSUPPORTED real-font metrics on headless GPUI text system: {error}; geometry assertions still run");
            }
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, &root.0)
            });
            editor.update(cx, |view, cx| {
                assert_eq!(view.browser.directory, root.0.join("samples"));
                view.state = Some(state);
                view.selected_track = Some(track);
                view.device_panel_active = true;
                view.clip_view_open = true;
                view.mode = super::super::super::super::EditorMode::Session;
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, root)
}

#[gpui::test]
fn eq_graph_dominates_body_without_permanent_control_rails(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, _editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [880.0, 1262.0, 1500.0] {
        visual.simulate_resize(size(px(width), px(650.0)));
        visual.run_until_parked();
        let body = size(px(600.0), px(168.0));
        let graph = visual
            .debug_bounds(selector("eq-response", device))
            .unwrap();
        assert!(
            graph.size.height >= body.height * 0.9,
            "graph must use the device height, not sit between rails: graph={graph:?} body={body:?}"
        );
        assert!(
            graph.size.width >= body.width * 0.7,
            "graph-dominant at {width}px"
        );
    }
}

#[gpui::test]
fn eq_axis_labels_are_logarithmic_nonoverlapping_and_keep_the_plot_hit_map(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let plot_bounds = visual
        .debug_bounds(Box::leak(
            format!("eq-response-{}-plot", device.value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("eq-response-{}-axis-x-0", device.value()).into_boxed_str(),
            ))
            .is_some(),
        "the 20 hz endpoint must not collide with a nonexistent previous label"
    );
    let mut labels = Vec::new();
    for i in 0..32 {
        if let Some(bounds) = visual.debug_bounds(Box::leak(
            format!("eq-response-{}-axis-x-{i}", device.value()).into_boxed_str(),
        )) {
            assert!(
                bounds.left() >= plot_bounds.left()
                    && bounds.right() <= plot_bounds.right() + px(1.0)
            );
            if let Some(right) = labels.last() {
                assert!(bounds.left() >= *right + px(3.0));
            }
            labels.push(bounds.right());
        }
    }
    assert!(
        labels.len() >= 6,
        "actual adaptive major labels remain readable"
    );
    let expected_hz = (20.0_f32 * 20_000.0).sqrt();
    visual.simulate_click(plot_bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let actual = editor.read_with(cx, |view, _| read(view, device).nodes[0].frequency_hz);
    assert!(
        (actual - expected_hz).abs() < 0.1,
        "curve/hit axes share the log midpoint: {actual}"
    );
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let labels_area = point(plot_bounds.center().x, plot_bounds.bottom() + px(10.0));
    visual.simulate_click(labels_area, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            read(view, device),
            before,
            "axis labels cannot create nodes"
        )
    });
}

#[gpui::test]
fn eq_default_spectrum_intent_and_explicit_toggle_do_not_edit_or_fake_audio(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let original = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            eq_panel::spectrum_intent_for_test(view, device),
            (true, true)
        );
        assert!(view.audio.is_none());
        assert!(view.device_panel.spectrum_results.is_empty());
    });
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", device))
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !eq_panel::spectrum_intent_for_test(view, device).0,
            "explicit toggle keeps this eq off"
        );
        assert_ne!(
            view.device_panel.message.as_deref(),
            Some("spectrum waits for available current project audio"),
            "explicit Off must not claim it is waiting for capture"
        );
        assert!(view.audio.is_none());
        assert!(view.device_panel.spectrum_results.is_empty());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.pending_requests.is_empty());
    });
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(eq_panel::spectrum_intent_for_test(view, device).0);
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum waits for available current project audio")
        );
        assert!(view.device_panel.spectrum_results.is_empty());
    });
    // The previously explicit On selection may activate once a valid fixture
    // publication becomes available, without retaining an obsolete wait error.
    let (control, _callback) = hex_audio::spectrum_target::channel();
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .eq_spectrum
                .test_channel
                .as_ref()
                .unwrap()
                .desired()
                .is_some()
        );
        assert!(view.device_panel.message.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
}

#[gpui::test]
fn eq_real_selectors_keep_popup_anchor_keyboard_cancel_and_canonical_history(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    eq_double_click(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let node = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
    let shape = node.parameter(hex_engine::eq::NodeParameter::Kind).unwrap();
    let name = |suffix: &str| {
        Box::leak(
            format!(
                "device-{}-parameter-{}{suffix}",
                device.value(),
                shape.value()
            )
            .into_boxed_str(),
        ) as &'static str
    };
    for width in [880.0, 1262.0, 1500.0] {
        visual.simulate_resize(size(px(width), px(650.0)));
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.device_panel
                .chain_window
                .scroll
                .set_offset(point(px(-600.0), px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| read(view, device));
        let field = visual.debug_bounds(name("")).unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.run_until_parked();
        let popup = visual
            .debug_bounds(name("-popup"))
            .expect("actual shared Selector popup");
        assert!(
            popup.left() >= px(0.0)
                && popup.right() <= px(width)
                && popup.size.width >= field.size.width
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds(name("-popup")).is_none());
        editor.read_with(cx, |view, _| {
            assert_eq!(
                read(view, device),
                before,
                "cancel preserves authored state"
            )
        });
        visual.simulate_keystrokes("enter down");
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) != before
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
    }
    enter_typed_value(
        &mut visual,
        device,
        node.parameter(hex_engine::eq::NodeParameter::Frequency)
            .unwrap(),
        "20000",
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].frequency_hz == 20_000.0
    });
    let before = editor.read_with(cx, |view, _| read(view, device));
    enter_typed_value(
        &mut visual,
        device,
        node.parameter(hex_engine::eq::NodeParameter::Frequency)
            .unwrap(),
        "bad",
    );
    visual.simulate_keystrokes("enter escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            read(view, device),
            before,
            "invalid draft and cancel preserve value"
        )
    });
}

#[gpui::test]
fn eq_shared_inline_width_preserves_default_and_rejects_invalid_geometry(cx: &mut TestAppContext) {
    use hex_widgets::{
        NumericControl, NumericControlConfig, NumericControlKind, NumericControlPresentation,
        NumericScale, NumericSpec,
    };
    let config = NumericControlConfig::new(
        "eq-width-check",
        theme::text("freq"),
        NumericControlKind::HorizontalSlider,
        NumericSpec::new(
            20.0,
            20_000.0,
            1000.0,
            NumericScale::Logarithmic,
            2,
            Some("hz"),
        )
        .unwrap(),
        1000.0,
        theme::device_style(theme::DeviceStyleRole::Readout),
    )
    .unwrap()
    .with_presentation(NumericControlPresentation::InlineValue);
    let window = cx.add_window(|_, cx| NumericControl::from_config(cx, config));
    let handle = *window;
    let control = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("eq-width-check").unwrap().size.width,
        px(46.0)
    );
    assert_eq!(
        visual
            .debug_bounds("eq-width-check-value")
            .unwrap()
            .size
            .width,
        px(44.0)
    );
    control.update(cx, |control, cx| {
        control.set_inline_value_width(104.0, cx).unwrap()
    });
    visual.run_until_parked();
    let outer = visual.debug_bounds("eq-width-check").unwrap();
    let field = visual.debug_bounds("eq-width-check-value").unwrap();
    assert_eq!(outer.size.width, px(104.0));
    assert_eq!(field.size.width, px(102.0));
    assert!(field.left() >= outer.left() && field.right() <= outer.right());
    for bad in [f32::NAN, f32::INFINITY, 0.0, 400.0] {
        control.update(cx, |control, cx| {
            assert!(control.set_inline_value_width(bad, cx).is_err())
        });
    }
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("eq-width-check"), Some(outer));
    control.read_with(cx, |control, _| assert_eq!(control.value(), 1000.0));
}

fn choose_option(
    visual: &mut VisualTestContext,
    device: DeviceId,
    parameter: hex_lattice_contract::ParameterId,
    index: usize,
) {
    let field = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-{}", device.value(), parameter.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(Box::leak(
            format!(
                "device-{}-parameter-{}-option-{index}",
                device.value(),
                parameter.value()
            )
            .into_boxed_str(),
        ))
        .expect("real shared selector option");
    visual.simulate_mouse_down(option.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(option.center(), MouseButton::Left, Modifiers::default());
}
fn measured_label(visual: &mut VisualTestContext, text: &str, font_size: f32) -> gpui::Pixels {
    visual.update(|window, _| {
        window
            .text_system()
            .shape_line(
                text.to_owned().into(),
                px(font_size),
                &[gpui::TextRun {
                    len: text.len(),
                    font: gpui::font(theme::FONT_FAMILY),
                    color: gpui::rgb(0xffffff).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
    })
}

#[gpui::test]
fn eq_long_numbers_channel_names_and_cut_slopes_fit_and_edit_through_real_controls(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    eq_double_click(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let node = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
    choose_option(&mut visual, device, hex_engine::eq::MODE, 1);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).mode == 1
    });
    let mode = visual
        .debug_bounds(Box::leak(
            format!(
                "device-{}-parameter-{}",
                device.value(),
                hex_engine::eq::MODE.value()
            )
            .into_boxed_str(),
        ))
        .unwrap();
    assert!(
        measured_label(&mut visual, "left/right", 8.0) + px(20.0) <= mode.size.width,
        "full channel text plus arrow/padding"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).mode == 0
    });
    let shape = node.parameter(hex_engine::eq::NodeParameter::Kind).unwrap();
    choose_option(&mut visual, device, shape, 2);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].kind == hex_engine::eq::NodeKind::HighShelf
    });
    for (field, text) in [
        (hex_engine::eq::NodeParameter::Frequency, "20000"),
        (hex_engine::eq::NodeParameter::Gain, "24"),
        (hex_engine::eq::NodeParameter::Q, "18"),
    ] {
        let parameter = node.parameter(field).unwrap();
        enter_typed_value(&mut visual, device, parameter, text);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, settled);
        visual.run_until_parked();
        let shown = editor.read_with(cx, |view, cx| {
            let ParameterControl::Float { entity, .. } =
                &view.device_panel.controls[&ParameterKey { device, parameter }]
            else {
                panic!("numeric field")
            };
            entity.read(cx).display_text().to_owned()
        });
        let bounds = visual
            .debug_bounds(Box::leak(
                format!(
                    "device-{}-parameter-{}-value",
                    device.value(),
                    parameter.value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        assert!(
            measured_label(&mut visual, &shown, 9.0) + px(6.0) <= bounds.size.width,
            "full value and units {shown:?}"
        );
    }
    editor.read_with(cx, |view, _| {
        let n = read(view, device).nodes[0];
        assert_eq!(n.frequency_hz, 20000.0);
        assert_eq!(n.gain_db, 24.0);
        assert_eq!(n.q, 18.0);
    });
    choose_option(&mut visual, device, shape, 4);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].kind == hex_engine::eq::NodeKind::HighCut
    });
    choose_option(
        &mut visual,
        device,
        node.parameter(hex_engine::eq::NodeParameter::Slope)
            .unwrap(),
        3,
    );
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 3
    });
}

#[gpui::test]
fn visible_eq_capture_survives_focus_on_another_visible_eq(cx: &mut TestAppContext) {
    let (service, _, track, first) = fixture(false);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let original = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let (control, mut callback) = hex_audio::spectrum_target::channel();
    editor.update(cx, |v, cx| {
        v.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(3200.0), px(650.0)));
    visual.run_until_parked();
    let focus = editor.update(cx, |v, cx| v.device_panel.chain_window.focus(first, cx));
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    let first_target = editor.read_with(cx, |v, _| {
        v.device_panel
            .eq_spectrum
            .test_channel
            .as_ref()
            .unwrap()
            .desired()
            .unwrap()
    });
    assert_eq!(first_target.identity.device_id, first.value());
    callback.begin_block();
    let focus = editor.update(cx, |v, cx| v.device_panel.chain_window.focus(second, cx));
    visual.update(|window, _| focus.focus(window));
    for _ in 0..2 {
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let viewport = visual.update(|window, _| window.viewport_size());
        for device in [first, second] {
            let graph = visual
                .debug_bounds(selector("eq-response", device))
                .unwrap();
            assert!(graph.left() >= px(0.0) && graph.right() <= viewport.width);
        }
        editor.read_with(cx, |v, _| {
            assert_eq!(v.device_panel.selected_device, Some(second));
            assert_eq!(v.state.as_ref().unwrap().project, original);
            assert_eq!(
                v.device_panel
                    .eq_spectrum
                    .test_channel
                    .as_ref()
                    .unwrap()
                    .desired(),
                Some(first_target),
                "focus must not replace another visible eq's live capture"
            );
        });
    }
}

// Exercise the real off-control FFT producer rather than inventing display
// bins. Native multi-target callback/PCM ownership is checked in hex-audio;
// this fixture checks its published-results consumer after completed paints.
fn analyzed_tone(
    target: hex_audio::spectrum_target::SpectrumTarget,
    frequency: f32,
    amplitude: f32,
    captured_at: std::time::Instant,
) -> hex_audio::spectrum_worker::PublishedSpectra {
    use hex_audio::spectrum_worker::{CaptureOutcome, SpectrumStamp, spawn_spectrum_worker};
    let stamp = SpectrumStamp {
        target: target.token,
        generation: target.project_generation,
        sample_rate_hz: target.sample_rate_hz,
        applied_revision: target.activation_floor,
    };
    let (mut capture, mut reader, reaper) = spawn_spectrum_worker(stamp).unwrap();
    for chunk in 0..16 {
        let frames: [[f32; 2]; 256] = std::array::from_fn(|i| {
            let phase = std::f32::consts::TAU * frequency * (chunk * 256 + i) as f32
                / target.sample_rate_hz as f32;
            [phase.sin() * amplitude; 2]
        });
        assert!(matches!(
            capture.submit_pair_at(stamp, &frames, &frames, captured_at),
            CaptureOutcome::Queued { .. }
        ));
    }
    let deadline = captured_at + std::time::Duration::from_secs(2);
    let pair = loop {
        if let Some(pair) = reader.try_latest_pair_at(captured_at) {
            break pair;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real FFT result missing"
        );
        std::thread::yield_now();
    };
    assert_eq!(pair.output.captured_at, captured_at);
    drop(capture);
    reaper.shutdown_and_join().unwrap();
    pair
}

#[gpui::test]
fn both_visible_eq_curves_advance_from_real_fft_results_without_focus_stealing(
    cx: &mut TestAppContext,
) {
    let (service, _, track, first) = fixture(false);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let original = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let (control, _callback) = hex_audio::spectrum_target::channel();
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(3200.0), px(650.0)));
    visual.run_until_parked();
    let focus = editor.update(cx, |v, cx| v.device_panel.chain_window.focus(second, cx));
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    let targets: Vec<_> = editor.read_with(cx, |v, _| {
        v.device_panel
            .eq_spectrum
            .test_channel
            .as_ref()
            .unwrap()
            .desired_targets()
            .iter()
            .map(|(_, t)| t)
            .collect()
    });
    assert_eq!(targets.len(), 2);
    let graphs = editor.read_with(cx, |v, _| {
        [
            eq_panel::graph_for_test(v, first),
            eq_panel::graph_for_test(v, second),
        ]
    });
    let mut previous: Option<Vec<Vec<[f32; 2]>>> = None;
    for amplitude in [0.05, 0.4] {
        // Fix the consumer clock, never the result bytes or capture timestamp.
        // Scheduling under parallel test load must not turn this presentation
        // assertion into a native 100ms deadline claim. Expiry is checked below.
        let captured_at = std::time::Instant::now();
        let pairs: Vec<_> = targets
            .iter()
            .enumerate()
            .map(|(i, target)| {
                analyzed_tone(
                    *target,
                    if i == 0 { 440.0 } else { 1760.0 },
                    amplitude,
                    captured_at,
                )
            })
            .collect();
        editor.update(cx, |v, cx| {
            v.device_panel.eq_spectrum.test_now = Some(captured_at);
            v.device_panel.spectrum_results.extend(
                pairs
                    .iter()
                    .cloned()
                    .map(|pair| (pair.output.stamp.target, pair)),
            );
            cx.notify();
        });
        for _ in 0..2 {
            visual.run_until_parked();
            editor.update(cx, |_, cx| cx.notify());
        }
        let curves: Vec<_> = graphs
            .iter()
            .map(|graph| {
                graph.read_with(cx, |g, _| {
                    let [output, input] = g.analyzer_layers();
                    let output = output.expect("fresh output survived completed repaint");
                    assert_eq!(
                        Some(output),
                        input,
                        "equal PCM has aligned input and output"
                    );
                    output.to_vec()
                })
            })
            .collect();
        assert_ne!(curves[0], curves[1], "independent tones remain distinct");
        if let Some(previous) = previous {
            for (current, previous) in curves.iter().zip(previous) {
                assert_ne!(*current, previous, "both visible EQs advance");
            }
        }
        previous = Some(curves);
        editor.update(cx, |v, cx| {
            v.device_panel.eq_spectrum.test_now = Some(
                captured_at
                    + hex_audio::spectrum_worker::DISPLAY_STALE_AFTER
                    + std::time::Duration::from_nanos(1),
            );
            cx.notify();
        });
        visual.run_until_parked();
        for graph in &graphs {
            graph.read_with(cx, |graph, _| {
                assert_eq!(graph.analyzer_layers(), [None, None], "stale curves clear");
            });
        }
        editor.read_with(cx, |v, _| {
            assert_eq!(v.device_panel.selected_device, Some(second));
            assert_eq!(v.state.as_ref().unwrap().project, original);
        });
    }
}

#[gpui::test]
fn visible_eq_bus_membership_follows_mounted_cards_and_panel_custody(cx: &mut TestAppContext) {
    for main in [false, true] {
        let cx = &mut cx.new_app();
        let (service, _, track, _) = fixture(false);
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "analyzer bus".into(),
            }))
            .unwrap();
        let mut state = receive_service_event(&service).state;
        let return_track = state.project.return_tracks()[0].id();
        for _ in 0..8 {
            let authored = EqState::default().encode().unwrap();
            let command = if main {
                Command::InsertMainDevice {
                    before: None,
                    state: authored,
                }
            } else {
                Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state: authored,
                }
            };
            service.submit(Request::Edit(command)).unwrap();
            state = receive_service_event(&service).state;
        }
        let original = state.project.clone();
        let devices: Vec<_> = if main {
            state.project.main_devices()
        } else {
            state.project.return_tracks()[0].devices()
        }
        .iter()
        .map(|d| d.id())
        .collect();
        let owner = if main {
            hex_engine::ProcessorChainOwner::Main
        } else {
            hex_engine::ProcessorChainOwner::Return(return_track.value())
        };
        let (handle, editor, _root) = bounded_window(cx, service, state, track);
        let (control, mut callback) = hex_audio::spectrum_target::channel();
        editor.update(cx, |v, cx| {
            v.device_panel.eq_spectrum.test_channel = Some(control);
            v.selected_track = None;
            v.focused_return = (!main).then_some(return_track);
            v.device_panel.main_selected = main.then_some(v.project_generation);
            cx.notify();
        });
        let visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(900.0), px(650.0)));
        visual.run_until_parked();
        callback.begin_block();
        let initial = *callback.current_targets().unwrap();
        assert!(!initial.is_empty() && initial.len() < devices.len());
        assert!(initial.iter().all(|(_, t)| t.identity.owner == owner));
        assert!(
            initial
                .iter()
                .any(|(_, t)| t.identity.device_id == devices[0].value())
        );
        editor.update(cx, |v, cx| {
            v.device_panel
                .chain_window
                .scroll
                .set_offset(point(px(-10000.0), px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        callback.begin_block();
        let end = callback.current_targets().unwrap();
        assert!(!end.is_empty() && end.len() < devices.len());
        assert!(
            end.iter()
                .any(|(_, t)| t.identity.device_id == devices[7].value())
        );
        assert!(
            !end.iter()
                .any(|(_, t)| t.identity.device_id == devices[0].value())
        );
        editor.update(cx, |v, cx| {
            v.clip_view_open = false;
            reconcile_eq_spectrum(v);
            assert!(
                v.device_panel
                    .eq_spectrum
                    .test_channel
                    .as_ref()
                    .unwrap()
                    .desired_targets()
                    .is_empty()
            );
            assert_eq!(v.state.as_ref().unwrap().project, original);
            cx.notify();
        });
        callback.begin_block();
        assert!(callback.current_targets().unwrap().is_empty());
    }
}

#[gpui::test]
fn eq_visible_membership_focus_off_owner_changes_and_removal_use_one_real_control_channel(
    cx: &mut TestAppContext,
) {
    let (service, _, track, first) = fixture(false);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "other owner".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(other) = event.created.unwrap() else {
        panic!("track");
    };
    let original = event.state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, event.state, track);
    let (control, mut callback) = hex_audio::spectrum_target::channel();
    editor.update(cx, |v, cx| {
        v.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(3200.0), px(650.0)));
    visual.run_until_parked();
    let desired = |editor: &Entity<EditorView>, cx: &TestAppContext| {
        editor.read_with(cx, |v, _| {
            *v.device_panel
                .eq_spectrum
                .test_channel
                .as_ref()
                .unwrap()
                .desired_targets()
        })
    };
    let admit =
        |editor: &Entity<EditorView>,
         cx: &mut TestAppContext,
         callback: &mut hex_audio::spectrum_target::SpectrumTargetCallback| {
            callback.begin_block();
            editor.update(cx, |v, _| {
                v.device_panel
                    .eq_spectrum
                    .test_channel
                    .as_mut()
                    .unwrap()
                    .retry_pending();
            });
            callback.begin_block();
        };
    let initial = desired(&editor, cx);
    assert_eq!(
        initial.len(),
        2,
        "mounted enabled EQs do not wait for selection"
    );
    editor.read_with(cx, |v, _| {
        assert!(
            v.device_panel.selected_device.is_none(),
            "mounting never selects a card"
        )
    });
    admit(&editor, cx, &mut callback);
    let hover = visual
        .debug_bounds(selector("eq8-spectrum", second))
        .unwrap();
    visual.simulate_mouse_move(hover.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        desired(&editor, cx),
        initial,
        "hover does not change membership"
    );
    // Wrong owner and generation cannot survive reconciliation. Restoring the
    // visible set is distinct from preserving the rejected foreign token.
    for (owner, generation_offset) in [(other.value(), 0), (track.value(), 1)] {
        editor.update(cx, |v, cx| {
            v.device_panel
                .eq_spectrum
                .test_channel
                .as_mut()
                .unwrap()
                .select(
                    hex_engine::PostNodeIdentity::new(
                        hex_engine::ProcessorChainOwner::Track(owner),
                        first.value(),
                        EQ_TYPE_ID,
                    ),
                    v.project_generation + generation_offset,
                    1,
                    48_000,
                )
                .unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let restored = desired(&editor, cx);
        assert_eq!(restored.len(), 2);
        assert!(restored.iter().all(|(_, t)| t.identity.owner
            == hex_engine::ProcessorChainOwner::Track(track.value())
            && t.project_generation == editor.read_with(cx, |v, _| v.project_generation)));
        admit(&editor, cx, &mut callback);
    }
    let accepted = desired(&editor, cx);
    for device in [first, second] {
        let focus = editor.update(cx, |v, cx| v.device_panel.chain_window.focus(device, cx));
        visual.update(|window, _| focus.focus(window));
        visual.run_until_parked();
        assert_eq!(desired(&editor, cx), accepted);
        assert_eq!(callback.current_targets(), Some(&accepted));
        editor.read_with(cx, |v, _| {
            assert_eq!(v.device_panel.selected_device, Some(device))
        });
    }
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", second))
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    admit(&editor, cx, &mut callback);
    let remaining = desired(&editor, cx);
    assert_eq!(remaining.len(), 1);
    let first_identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track.value()),
        first.value(),
        EQ_TYPE_ID,
    );
    assert_eq!(
        remaining.find(first_identity),
        accepted.find(first_identity)
    );
    editor.read_with(cx, |v, _| {
        assert!(!eq_panel::spectrum_intent_for_test(v, second).0);
        assert_eq!(v.state.as_ref().unwrap().project, original);
        assert!(v.pending_requests.is_empty());
    });
    for owner in [other, track] {
        editor.update(cx, |v, cx| {
            v.selected_track = Some(owner);
            cx.notify();
        });
        visual.run_until_parked();
        admit(&editor, cx, &mut callback);
        assert_eq!(desired(&editor, cx).len(), usize::from(owner == track));
    }
    editor.read_with(cx, |v, _| {
        assert!(
            !eq_panel::spectrum_intent_for_test(v, second).0,
            "explicit Off survives owner changes"
        )
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RemoveDevice(first))))
    });
    pump_until(&editor, cx, |v| {
        settled(v) && v.state.as_ref().unwrap().project.device(first).is_none()
    });
    visual.run_until_parked();
    admit(&editor, cx, &mut callback);
    assert!(desired(&editor, cx).is_empty());
    assert!(callback.current().is_none());
}

#[gpui::test]
fn eq_analyzer_status_closed_public_client_is_visible_without_changing_intent(
    cx: &mut TestAppContext,
) {
    let _status_fixture_guard = status_fixture_guard();
    let (service, state, track, device) = fixture(false);
    let original = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    // Existing instrumented constructor owns real endpoint guards; it never
    // opens CPAL, queries a device, renders audio or requests OS permission.
    let (mut output, backend) = headless_status_output();
    let mut client = output.take_client().unwrap();
    client.close();
    let identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track.value()),
        device.value(),
        EQ_TYPE_ID,
    );
    assert_eq!(
        client.spectrum_select(identity, 1, 1, 48_000),
        Err(hex_audio::spectrum_target::TargetError::OutputClosed),
        "the public guarded client, not a fabricated error, must refuse"
    );
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_client = Some(client);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let focus = editor.update(cx, |view, cx| {
        view.device_panel.chain_window.focus(device, cx)
    });
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device));
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum unavailable: audio output is closed")
        );
        assert!(eq_panel::spectrum_intent_for_test(view, device).0);
        assert!(view.device_panel.spectrum_results.is_empty());
        assert!(view.audio.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.pending_requests.is_empty());
    });
    editor.update(cx, |view, _| {
        view.device_panel.eq_spectrum.test_client.take();
    });
    drop(backend);
    drop(output);
}

#[gpui::test]
fn eq_analyzer_off_focus_retains_other_waiting_intent_and_unrelated_errors(
    cx: &mut TestAppContext,
) {
    let (service, _, track, first) = fixture(false);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let original = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1800.0), px(650.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel
            .chain_window
            .scroll
            .set_offset(point(px(-1200.0), px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", second))
        .unwrap();
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(toggle.center().x > px(0.0) && toggle.center().x < viewport.width);
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(!editor.read_with(cx, |view, _| {
        eq_panel::spectrum_intent_for_test(view, second).0
    }));
    editor.update(cx, |view, cx| {
        view.device_panel
            .chain_window
            .scroll
            .set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", first))
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(eq_panel::spectrum_intent_for_test(view, first).0);
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum waits for available current project audio")
        );
    });
    let focus = editor.update(cx, |view, cx| {
        view.device_panel.chain_window.focus(second, cx)
    });
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(second));
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum waits for available current project audio"),
            "focus on an Off EQ cannot hide another mounted EQ's pending intent"
        );
    });
    editor.update(cx, |view, cx| {
        view.device_panel.message = Some("could not save project".into());
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("could not save project")
        );
        assert!(!eq_panel::spectrum_intent_for_test(view, second).0);
        assert!(view.device_panel.spectrum_results.is_empty());
        assert!(view.audio.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.pending_requests.is_empty());
    });
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn eq_native_bundled_font_test_context(cx: &mut TestAppContext) {
    cx.use_native_text_system().unwrap();
    cx.update(|cx| {
        theme::load_fonts(cx).expect("actual embedded ioskeley family must register");
        assert!(
            cx.text_system()
                .all_font_names()
                .iter()
                .any(|name| name == theme::FONT_FAMILY)
        );
    });
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let single = measured_label(&mut visual, "q", 9.0);
    let multiple = measured_label(&mut visual, "20000.00 hz", 9.0);
    assert!(single > px(0.0) && multiple > single * 8.0);
    assert!(
        cx.use_native_text_system().is_err(),
        "cannot replace fonts beneath live windows"
    );
}

#[gpui::test]
fn eq_production_rail_screenshot_sizes_keep_controls_disjoint_and_slope_popup_operable(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.5, 0.5);
    eq_double_click(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let node = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
    choose_option(
        &mut visual,
        device,
        node.parameter(hex_engine::eq::NodeParameter::Kind).unwrap(),
        4,
    );
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].kind == hex_engine::eq::NodeKind::HighCut
    });
    let parameter_name = |id: hex_lattice_contract::ParameterId, suffix: &str| {
        Box::leak(
            format!("device-{}-parameter-{}{suffix}", device.value(), id.value()).into_boxed_str(),
        ) as &'static str
    };
    // 1314x426 is also tested as a deliberately short viewport stress case;
    // the actual production minimum is 960x600, not inferred from a cropped image.
    for (width, height) in [
        (960.0, 600.0),
        (1314.0, 426.0),
        (1314.0, 600.0),
        (1800.0, 900.0),
    ] {
        visual.simulate_resize(size(px(width), px(height)));
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.device_panel
                .chain_window
                .scroll
                .set_offset(point(px(-600.0), px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        let sidebar = visual.debug_bounds(selector("eq-sidebar", device)).unwrap();
        let graph = visual
            .debug_bounds(selector("eq-response", device))
            .unwrap();
        assert!(
            graph.right() <= sidebar.left(),
            "plot and controls have separate geometry"
        );
        assert!(
            graph.size.height >= px(150.0),
            "device plot keeps practical vertical room"
        );
        let mut rows: Vec<gpui::Bounds<gpui::Pixels>> = Vec::new();
        for field in [
            hex_engine::eq::NodeParameter::Enabled,
            hex_engine::eq::NodeParameter::Kind,
            hex_engine::eq::NodeParameter::Frequency,
            hex_engine::eq::NodeParameter::Gain,
            hex_engine::eq::NodeParameter::Q,
            hex_engine::eq::NodeParameter::Slope,
        ] {
            let id = node.parameter(field).unwrap();
            let row = visual
                .debug_bounds(Box::leak(
                    format!("eq-control-{}-{}", device.value(), id.value()).into_boxed_str(),
                ))
                .unwrap();
            assert!(
                row.left() >= sidebar.left() && row.right() <= sidebar.right(),
                "complete row within sidebar: {row:?} / {sidebar:?}"
            );
            let input = visual.debug_bounds(parameter_name(id, "")).unwrap();
            assert!(
                input.left() >= row.left()
                    && input.right() <= row.right()
                    && input.top() >= row.top()
                    && input.bottom() <= row.bottom(),
                "real control within independently sized row: {input:?} / {row:?}"
            );
            for previous in &rows {
                assert!(
                    row.right() <= previous.left()
                        || previous.right() <= row.left()
                        || row.bottom() <= previous.top()
                        || previous.bottom() <= row.top(),
                    "closed controls cannot overlap: {row:?} / {previous:?}"
                );
            }
            rows.push(row);
        }
        if height < 600.0 {
            // This is deliberately outside the actual WindowOptions minimum.
            // A screenshot crop is not evidence of a 426 logical-pixel app
            // window. Retain the bounds check, but do not claim offscreen input
            // is operable or weaken the supported-window assertions below.
            assert!(sidebar.bottom() > px(height));
            eprintln!(
                "UNSUPPORTED viewport stress {width}x{height}: below production 960x600 minimum; cropped rail geometry is disjoint, input/visual acceptance not established"
            );
            continue;
        }
        let slope = node
            .parameter(hex_engine::eq::NodeParameter::Slope)
            .unwrap();
        let field = visual.debug_bounds(parameter_name(slope, "")).unwrap();
        assert!(
            field.center().x > px(0.0)
                && field.center().x < px(width)
                && field.center().y > px(0.0)
                && field.center().y < px(height),
            "slope hit target visible in viewport {width}x{height}: {field:?}; sidebar={sidebar:?}"
        );
        let before = editor.read_with(cx, |view, _| read(view, device));
        visual.simulate_click(field.center(), Modifiers::default());
        visual.run_until_parked();
        let popup = visual
            .debug_bounds(parameter_name(slope, "-popup"))
            .expect("actual slope popup opened from hit target");
        assert!(
            popup.left() >= px(0.0)
                && popup.right() <= px(width)
                && popup.top() >= px(0.0)
                && popup.bottom() <= px(height)
        );
        for (index, label) in ["12 db/oct", "24 db/oct", "36 db/oct", "48 db/oct"]
            .into_iter()
            .enumerate()
        {
            let option = visual
                .debug_bounds(parameter_name(slope, &format!("-option-{index}")))
                .unwrap();
            assert!(option.left() >= popup.left() && option.right() <= popup.right());
            assert!(measured_label(&mut visual, label, 8.0) + px(8.0) <= option.size.width);
        }
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(parameter_name(slope, "-popup"))
                .is_none()
        );
        editor.read_with(cx, |view, _| assert_eq!(read(view, device), before));
    }
}

#[gpui::test]
fn eq_review_selection_of_non_eq_retains_visible_public_capture_target(cx: &mut TestAppContext) {
    let (service, _, track, eq) = fixture(false);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.utility",
            )),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let utility = state.project.track(track).unwrap().devices()[1].id();
    let before = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let (control, mut callback) = hex_audio::spectrum_target::channel();
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1800.0), px(650.0)));
    visual.run_until_parked();
    let focus = editor.update(cx, |view, cx| view.device_panel.chain_window.focus(eq, cx));
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    let target = editor
        .read_with(cx, |view, _| {
            view.device_panel
                .eq_spectrum
                .test_channel
                .as_ref()
                .unwrap()
                .desired()
        })
        .expect("focused eq defaults on");
    callback.begin_block();
    assert_eq!(callback.current(), Some(target));
    let header = visual
        .debug_bounds(selector("device-drag-title", utility))
        .unwrap();
    visual.simulate_click(header.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(utility));
        assert_eq!(
            view.device_panel
                .eq_spectrum
                .test_channel
                .as_ref()
                .unwrap()
                .desired(),
            Some(target),
            "selecting another card cannot retire a mounted EQ's capture"
        );
        assert!(view.device_panel.spectrum_results.is_empty());
        assert_eq!(view.state.as_ref().unwrap().project, before);
        assert!(view.audio.is_none());
    });
    assert_eq!(
        callback.current(),
        Some(target),
        "the admitted EQ remains active"
    );
}

#[gpui::test]
fn eq_review_new_and_open_empty_projects_retire_only_owned_analyzer_status(
    cx: &mut TestAppContext,
) {
    let _status_fixture_guard = status_fixture_guard();
    let mut completed = 0;
    for open in [false, true] {
        for status in ["waiting", "closed", "unrelated"] {
            // Every variant owns an independent app. bounded_window selects the
            // real native text system before that app's first window; earlier
            // variants must never force a replacement under existing windows.
            let cx = &mut cx.new_app();
            eprintln!("EQ document status variant: Open={open}, old status={status}");
            let (service, state, track, device) = fixture(false);
            let (handle, editor, root) = bounded_window(cx, service, state, track);
            let mut backend_owners = None;
            if status == "closed" {
                let (mut output, backend) = headless_status_output();
                let mut client = output.take_client().unwrap();
                client.close();
                editor.update(cx, |view, _| {
                    view.device_panel.eq_spectrum.test_client = Some(client)
                });
                backend_owners = Some((output, backend));
            }
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.simulate_resize(size(px(1500.0), px(650.0)));
            visual.run_until_parked();
            let focus = editor.update(cx, |view, cx| {
                view.device_panel.chain_window.focus(device, cx)
            });
            visual.update(|window, _| focus.focus(window));
            visual.run_until_parked();
            if status != "closed" {
                let toggle = visual
                    .debug_bounds(selector("eq8-spectrum", device))
                    .unwrap();
                visual.simulate_click(toggle.center(), Modifiers::default());
                visual.simulate_click(toggle.center(), Modifiers::default());
                visual.run_until_parked();
            }
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    view.device_panel.message.as_deref(),
                    Some(if status == "closed" {
                        "spectrum unavailable: audio output is closed"
                    } else {
                        "spectrum waits for available current project audio"
                    })
                )
            });
            let request = if open {
                let path = root.0.join("empty.hex");
                let fresh = Service::new(48_000, 8, 8_000_000).unwrap();
                let _ = receive_service_event(&fresh);
                fresh.submit(Request::SaveNew(path.clone())).unwrap();
                assert!(receive_service_event(&fresh).error.is_none());
                Request::Open(path)
            } else {
                Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                }
            };
            let generation = editor.update(cx, |view, _| {
                if status == "unrelated" {
                    view.device_panel.message = Some("could not save project".into());
                }
                assert!(view.enqueue(request));
                view.project_generation
            });
            pump_until(&editor, cx, |view| {
                settled(view) && view.project_generation > generation
            });
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.state.as_ref().unwrap().project.tracks().is_empty());
                assert!(view.device_panel.selected_device.is_none());
                assert_eq!(
                    view.device_panel.message.as_deref(),
                    if status == "unrelated" {
                        Some("could not save project")
                    } else {
                        None
                    },
                    "Open={open}, old status={status}"
                );
                assert!(view.audio.is_none(), "fixture must stay headless");
            });
            editor.update(cx, |view, _| {
                view.device_panel.eq_spectrum.test_client.take();
            });
            drop(backend_owners);
            completed += 1;
        }
    }
    assert_eq!(completed, 6, "all New/Open status variants completed");
}

#[gpui::test]
fn eq_review_boundary_node_outer_hit_wins_but_blank_axis_never_creates(cx: &mut TestAppContext) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let center = plot(&mut visual, device, 0.5, 0.5);
    eq_double_click(&mut visual, center);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    let initial = editor.read_with(cx, |view, _| read(view, device));
    let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<
        hex_widgets::ResponseGraphEvent,
    >::new()));
    let captured = events.clone();
    let _subscription = graph.update(cx, |_, cx| {
        cx.subscribe(
            &graph,
            move |_, _, event: &hex_widgets::ResponseGraphEvent, _| {
                captured.borrow_mut().push(*event)
            },
        )
    });
    for (frequency, gain, x, y, outward) in [
        (20.0, 0.0, 0.0, 0.5, point(px(-4.0), px(0.0))),
        (20_000.0, 0.0, 1.0, 0.5, point(px(4.0), px(0.0))),
        (
            initial.nodes[0].frequency_hz,
            -24.0,
            0.5,
            0.0,
            point(px(0.0), px(4.0)),
        ),
        (
            initial.nodes[0].frequency_hz,
            24.0,
            0.5,
            1.0,
            point(px(0.0), px(-4.0)),
        ),
    ] {
        let mut expected = initial.clone();
        expected.nodes[0].frequency_hz = frequency;
        expected.nodes[0].gain_db = gain;
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::ReplaceDeviceState {
                device,
                state: expected.encode().unwrap()
            })))
        });
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == expected
        });
        visual.run_until_parked();
        events.borrow_mut().clear();
        let hit = plot(&mut visual, device, x, y) + outward;
        visual.simulate_click(hit, Modifiers::default());
        visual.run_until_parked();
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| event.node_id == expected.nodes[0].id.value()
                    && event.phase == hex_widgets::ResponseGraphPhase::Press),
            "outer hit frequency{frequency}/gain{gain}"
        );
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected);
        let moved = hit - outward.map(|value| value * 6.0);
        visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) != expected
        });
        let dragged = editor.read_with(cx, |view, _| read(view, device));
        assert_eq!(dragged.nodes.len(), 1);
        assert_eq!(dragged.nodes[0].id, expected.nodes[0].id);
        assert_eq!(dragged.nodes[0].kind, expected.nodes[0].kind);
        assert_eq!(dragged.nodes[0].q, expected.nodes[0].q);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == expected
        });
        visual.run_until_parked();
        events.borrow_mut().clear();
        let blank = plot(&mut visual, device, 0.0, 0.25) + point(px(-4.0), px(0.0));
        eq_double_click(&mut visual, blank);
        visual.run_until_parked();
        assert!(
            events.borrow().is_empty(),
            "blank axis is not a creation or node hit"
        );
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected);
    }
}

#[gpui::test]
fn eq_status_review_setup_rate_and_pending_contexts_remain_distinct(cx: &mut TestAppContext) {
    let _status_fixture_guard = status_fixture_guard();
    let mut completed = 0;
    for (rate, setup, expected) in [
        (
            48_000,
            Some("fixture spectrum setup failed"),
            Some("spectrum unavailable: fixture spectrum setup failed"),
        ),
        (
            7_999,
            None,
            Some("spectrum unavailable: unsupported audio sample rate"),
        ),
        (48_000, None, None),
    ] {
        let cx = &mut cx.new_app();
        eprintln!("EQ status context variant rate={rate}, setup={setup:?}");
        let (service, state, track, device) = fixture(false);
        let before = state.project.clone();
        let (handle, editor, _root) = bounded_window(cx, service, state, track);
        // Existing guarded public control only: no CPAL, OS device, callback
        // render, fabricated frame or new backend implementation. Metadata is
        // the real EditorAudio carrier received by the UI after output setup.
        let (mut output, backend) = headless_status_output();
        let mut client = output.take_client().unwrap();
        if rate == 7_999 {
            assert_eq!(
                client.spectrum_select(
                    hex_engine::PostNodeIdentity::new(
                        hex_engine::ProcessorChainOwner::Track(track.value()),
                        device.value(),
                        EQ_TYPE_ID
                    ),
                    1,
                    1,
                    rate
                ),
                Err(hex_audio::spectrum_target::TargetError::UnsupportedSampleRate(rate))
            );
        }
        editor.update(cx, |view, cx| {
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: client,
                sample_rate: rate,
                label: "headless status fixture".into(),
                spectrum_error: setup.map(str::to_owned),
                mode: hex_session::DocumentAudioMode::Resident,
            });
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.simulate_resize(size(px(1500.0), px(650.0)));
        visual.run_until_parked();
        let focus = editor.update(cx, |view, cx| {
            view.device_panel.chain_window.focus(device, cx)
        });
        visual.update(|window, _| focus.focus(window));
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert_eq!(
                view.device_panel.message.as_deref(),
                expected,
                "rate={rate}, setup={setup:?}"
            );
            assert!(
                view.audio
                    .as_ref()
                    .unwrap()
                    .control
                    .spectrum_desired()
                    .is_none()
            );
            assert_eq!(view.state.as_ref().unwrap().project, before);
            assert!(view.device_panel.spectrum_results.is_empty());
        });
        editor.update(cx, |view, _| view.audio.take());
        drop(backend);
        drop(output);
        completed += 1;
    }
    assert_eq!(
        completed, 3,
        "setup, unsupported rate and pending variants all completed"
    );
}

#[gpui::test]
fn eq_status_review_repeated_closed_endpoint_does_not_erase_newer_save_error(
    cx: &mut TestAppContext,
) {
    let _status_fixture_guard = status_fixture_guard();
    let (service, state, track, device) = fixture(false);
    let before = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let (mut output, backend) = headless_status_output();
    let mut client = output.take_client().unwrap();
    client.close();
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_client = Some(client);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let focus = editor.update(cx, |view, cx| {
        view.device_panel.chain_window.focus(device, cx)
    });
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum unavailable: audio output is closed")
        )
    });
    for iteration in 0..3 {
        editor.update(cx, |view, cx| {
            if iteration == 0 {
                view.device_panel.message = Some("could not save project: disk full".into());
            }
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.message.as_deref(),
                Some("could not save project: disk full"),
                "actual repeated render {iteration}"
            );
            assert!(eq_panel::spectrum_intent_for_test(view, device).0);
            assert_eq!(view.state.as_ref().unwrap().project, before);
            assert!(view.audio.is_none());
        });
    }
    // Once the unrelated owner dismisses its error, current analyzer failure
    // must still be reportable; preserving save feedback is not hiding faults.
    editor.update(cx, |view, cx| {
        view.device_panel.message = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.message.as_deref(),
            Some("spectrum unavailable: audio output is closed")
        )
    });
    editor.update(cx, |view, _| {
        view.device_panel.eq_spectrum.test_client.take()
    });
    drop(backend);
    drop(output);
}

#[gpui::test]
fn eq_status_review_off_sparse_preserves_legacy_public_target_and_clears_own(
    cx: &mut TestAppContext,
) {
    let (service, _, track, legacy) = fixture(true);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let sparse = state.project.track(track).unwrap().devices()[1].id();
    let before = state.project.clone();
    let (handle, editor, _root) = bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1800.0), px(650.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel
            .chain_window
            .scroll
            .set_offset(point(px(-1200.0), px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", sparse))
        .unwrap();
    let viewport = visual.update(|window, _| window.viewport_size());
    assert!(toggle.center().x > px(0.0) && toggle.center().x < viewport.width);
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(!editor.read_with(cx, |view, _| {
        eq_panel::spectrum_intent_for_test(view, sparse).0
    }));
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let (mut control, mut callback) = hex_audio::spectrum_target::channel();
    let identity = hex_engine::PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(track.value()),
        legacy.value(),
        hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
    );
    let (legacy_target, _) = control.select(identity, generation, 1, 48_000).unwrap();
    callback.begin_block();
    assert_eq!(callback.current(), Some(legacy_target));
    editor.update(cx, |view, cx| {
        view.device_panel.eq_spectrum.test_channel = Some(control);
        cx.notify();
    });
    let focus = editor.update(cx, |view, cx| {
        view.device_panel.chain_window.focus(sparse, cx)
    });
    visual.update(|window, _| focus.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(sparse));
        assert_eq!(
            view.device_panel
                .eq_spectrum
                .test_channel
                .as_ref()
                .unwrap()
                .desired(),
            Some(legacy_target),
            "Off sparse selection cannot clear an independent legacy EQ8"
        );
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
    assert_eq!(
        callback.current(),
        Some(legacy_target),
        "accepted legacy tap remains admitted"
    );
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", sparse))
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    callback.begin_block();
    let own = callback
        .current_targets()
        .unwrap()
        .iter()
        .find_map(|(_, target)| (target.identity.device_id == sparse.value()).then_some(target))
        .expect("sparse On adds an independent target");
    assert_eq!(callback.current_targets().unwrap().len(), 2);
    assert_eq!(own.identity.device_id, sparse.value());
    assert_eq!(own.identity.device_type, EQ_TYPE_ID);
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let targets = view
            .device_panel
            .eq_spectrum
            .test_channel
            .as_ref()
            .unwrap()
            .desired_targets();
        assert_eq!(targets.len(), 1, "Off removes only its own target");
        assert_eq!(
            targets.find(identity).map(|(_, target)| target),
            Some(legacy_target)
        );
        assert!(view.audio.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
    assert!(
        callback.current().is_none(),
        "own admitted tap invalidated immediately"
    );
}

// Exercise the production recovery transition using a real guarded headless
// control; Service::new has no OS audio adapter. No callback render is needed.
fn recovery_retires_endpoint_status(cx: &mut TestAppContext, setup_failure: bool) {
    let _guard = status_fixture_guard();
    let mut completed = 0;
    for unrelated in [false, true] {
        let cx = &mut cx.new_app();
        let (service, state, track, device) = fixture(false);
        let before = state.project.clone();
        let (handle, editor, _root) = bounded_window(cx, service, state, track);
        let (mut output, backend) = headless_status_output();
        let mut client = output.take_client().unwrap();
        if !setup_failure {
            client.close();
        }
        editor.update(cx, |view, cx| {
            let project = crate::editor::audio_admission::ProjectRevision {
                generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
            };
            view.audio_admission.canonical_revision(project);
            view.audio_admission.published(project, 1);
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: client,
                sample_rate: 48_000,
                label: "headless recovery fixture".into(),
                spectrum_error: setup_failure.then(|| "fixture setup failed".into()),
                mode: hex_session::DocumentAudioMode::Resident,
            });
            view.device_panel.selected_device = Some(device);
            cx.notify();
        });
        let visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1500.0), px(650.0)));
        visual.run_until_parked();
        let expected = if setup_failure {
            "spectrum unavailable: fixture setup failed"
        } else {
            "spectrum unavailable: audio output is closed"
        };
        let generation = editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.message.as_deref(), Some(expected));
            view.project_generation
        });
        editor.update(cx, |view, cx| {
            if unrelated {
                view.device_panel.message = Some("could not save project: disk full".into());
            }
            view.recover_audio();
            assert!(
                view.audio.is_none(),
                "real recovery retires the old endpoint"
            );
            cx.notify();
        });
        visual.run_until_parked();
        let expected_pending = unrelated.then_some("could not save project: disk full");
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation);
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert_eq!(
                view.device_panel.message.as_deref(),
                expected_pending,
                "retired endpoint: setup={setup_failure}, unrelated={unrelated}"
            );
            assert_eq!(view.state.as_ref().unwrap().project, before);
            assert!(view.device_panel.spectrum_results.is_empty());
            assert!(eq_panel::spectrum_intent_for_test(view, device).0);
        });
        drop(backend);
        drop(output);
        // A replacement endpoint is present but its project graph has not been
        // published. The previous failure must not revive on repeated paint.
        let (mut output, backend) = headless_status_output();
        let client = output.take_client().unwrap();
        editor.update(cx, |view, cx| {
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: client,
                sample_rate: 48_000,
                label: "headless pending publication".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            cx.notify();
        });
        for _ in 0..3 {
            visual.run_until_parked();
            editor.update(cx, |view, cx| {
                assert_eq!(view.device_panel.message.as_deref(), expected_pending);
                assert!(
                    view.audio
                        .as_ref()
                        .unwrap()
                        .control
                        .spectrum_desired()
                        .is_none()
                );
                assert_eq!(view.state.as_ref().unwrap().project, before);
                cx.notify();
            });
        }
        editor.update(cx, |view, _| view.audio.take());
        drop(backend);
        drop(output);
        completed += 1;
        eprintln!("recovery status completed setup={setup_failure}, unrelated={unrelated}");
    }
    assert_eq!(
        completed, 2,
        "owned and unrelated status variants completed"
    );
}

#[gpui::test]
fn eq_status_recovery_retires_setup_failure_while_preserving_newer_error(cx: &mut TestAppContext) {
    recovery_retires_endpoint_status(cx, true);
}

#[gpui::test]
fn eq_status_recovery_retires_closed_endpoint_while_preserving_newer_error(
    cx: &mut TestAppContext,
) {
    recovery_retires_endpoint_status(cx, false);
}
