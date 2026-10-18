use super::*;

pub(super) struct BrowserRoot(std::path::PathBuf);
impl Drop for BrowserRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
    let root = BrowserRoot(std::env::temp_dir().join(format!(
        "hex-eq-layout-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir(&root.0).unwrap();
    std::fs::create_dir(root.0.join("samples")).unwrap();
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
        assert!(view.device_panel.spectrum_result.is_none());
    });
    let toggle = visual
        .debug_bounds(selector("eq8-spectrum", device))
        .unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !eq_panel::spectrum_intent_for_test(view, device).0,
            "explicit toggle retires automatic intent"
        );
        assert!(view.audio.is_none());
        assert!(view.device_panel.spectrum_result.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.pending_requests.is_empty());
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
    visual.simulate_click(position, Modifiers::default());
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
        NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlKind,
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
    .with_appearance(NumericControlAppearance::InlineValue);
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
