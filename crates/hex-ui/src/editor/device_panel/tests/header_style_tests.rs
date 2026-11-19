use super::super::test_support::RestoreTheme;
use super::*;

#[gpui::test]
fn retained_source_path_indicator_is_rendered_with_the_existing_chain(cx: &mut TestAppContext) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = model
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let Created::Device(effect) = model
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let instrument = model
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    for retained in [false, true] {
        let mut snapshot = model.project().snapshot();
        if retained {
            snapshot.retained_source_paths.push(track.value());
        }
        let project = Project::from_snapshot(snapshot).unwrap();
        let (window, editor) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        editor.update(cx, |view, cx| {
            view.selected_track = Some(track);
            view.device_panel_active = true;
            view.clip_view_open = true;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("retained-source-path").is_some(),
            retained
        );
        assert!(visual.debug_bounds("device-chain-scroll").is_some());
        let selector =
            Box::leak(format!("instrument-device-{}", instrument.value()).into_boxed_str());
        assert!(visual.debug_bounds(selector).is_some());
        let selector = Box::leak(format!("effect-device-{}", effect.value()).into_boxed_str());
        assert!(visual.debug_bounds(selector).is_some());
    }
}

fn assert_outline(
    visual: &mut VisualTestContext,
    device: DeviceId,
    selected: bool,
) -> Bounds<Pixels> {
    let selector = Box::leak(format!("device-shell-{}", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("visible device shell");
    let scale = visual.update(|window, _| window.scale_factor());
    let (color, widths, _) = visual
        .rendered_border(selector)
        .expect("one completed shell stroke");
    assert_eq!(
        color,
        gpui::Hsla::from(rgb(theme::color(if selected {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Border
        }))),
        "only the selected shell uses the highlight token: device={device:?}, selected={selected}"
    );
    assert_ne!(
        color,
        gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Surface))),
        "body-colored paint cannot show an outline"
    );
    let stroke = px(hex_widgets::DEVICE_FRAME_BORDER_WIDTH).scale(scale);
    assert_eq!(
        [widths.top, widths.right, widths.bottom, widths.left],
        [stroke; 4]
    );
    let (corners, _) = visual
        .rendered_surface_geometry(selector)
        .expect("rounded shell fill");
    let radius = px(hex_widgets::DEVICE_CORNER_RADIUS).scale(scale);
    assert_eq!(
        [
            corners.top_left,
            corners.top_right,
            corners.bottom_right,
            corners.bottom_left
        ],
        [radius; 4]
    );
    assert_eq!(
        visual.rendered_background(selector),
        Some(rgb(theme::color(theme::PaletteRole::Surface)).into()),
        "selection must keep the body neutral"
    );
    let physical = bounds.scale(scale);
    // GPUI can split a rounded stroke into several masked scene fragments.
    // Inspect their completed coverage rather than requiring the last fragment
    // (often only the rounded top strip) to cover the whole frame.
    let layers = visual.rendered_quad_paint_layers(selector, true);
    let pane = visual.debug_bounds("device-panel");
    let rail = visual.debug_bounds("device-chain-scroll");
    assert_eq!(
        visual.rendered_background("device-chain-scroll"),
        Some(rgb(theme::color(theme::PaletteRole::Border)).into()),
        "the shared rail stays recessed behind the neutral device shell"
    );
    assert!(
        layers
            .iter()
            .any(|(_, mask)| mask.top() <= physical.top()
                && mask.bottom() >= physical.top() + stroke)
            && layers
                .iter()
                .any(|(_, mask)| mask.top() <= physical.bottom() - stroke
                    && mask.bottom() >= physical.bottom()),
        "both horizontal strokes must fit the actual pane across scene fragments: {layers:?}, shell={physical:?}, pane={pane:?}, rail={rail:?}"
    );
    bounds
}

#[gpui::test]
fn device_shell_outline_is_visible_rounded_and_tracks_selection_across_owners(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, height) in [(1280.0, 700.0), (960.0, 600.0)] {
        editor.update(cx, |view, cx| {
            view.device_panel.select_only(None);
            cx.notify();
        });
        visual.simulate_resize(size(px(width), px(height)));
        visual.run_until_parked();
        let before = [instrument, effect].map(|id| assert_outline(&mut visual, id, false));
        editor.update(cx, |view, cx| {
            view.device_panel.selected_device = Some(effect);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| assert!(view.device_panel.is_selected(effect)));
        for (id, previous) in [instrument, effect].into_iter().zip(before) {
            assert_eq!(
                assert_outline(&mut visual, id, id == effect).size,
                previous.size,
                "selection cannot change shell metrics"
            );
        }
    }
    theme::apply_preferences(hex_preferences::ThemePreferences {
        background: hex_preferences::RgbColor::new(225, 229, 238),
        text: hex_preferences::RgbColor::new(24, 27, 34),
        primary: hex_preferences::RgbColor::new(145, 25, 45),
        secondary: hex_preferences::RgbColor::new(16, 70, 145),
    });
    let (service, state, track, return_id, device, _) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_id);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(960.0), px(600.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel.select_only(Some(device));
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.device_panel.is_selected(device)));
    assert_outline(&mut visual, device, true);
}
