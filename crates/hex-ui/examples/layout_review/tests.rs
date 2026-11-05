use super::*;
use gpui::{Focusable, Modifiers, MouseButton, TestAppContext, VisualTestContext, size};

struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}
fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
fn open(cx: &mut TestAppContext, entry: Entry) -> (gpui::AnyWindowHandle, Entity<Gallery>) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    let handle = cx.add_window(|_, cx| {
        let mut view = Gallery::new(cx);
        view.entry = entry;
        view.ensure_fixture(cx);
        view
    });
    let window = *handle;
    let view = handle.root(cx).expect("gallery root");
    (window, view)
}
fn choose(visual: &mut VisualTestContext, view: &Entity<Gallery>, entry: Entry) {
    visual.update(|window, cx| view.update(cx, |view, cx| view.choose_entry(entry, window, cx)));
    visual.run_until_parked();
}
fn number_id(entry: Entry, index: usize, suffix: &str) -> &'static str {
    selector(format!("fixture-{}-numeric-{index}{suffix}", entry.key()))
}

#[gpui::test]
fn catalog_fixture_track_return_labels_reuse_production_geometry_in_bounded_viewports(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, alternate) in [(760.0, false), (1320.0, true)] {
        view.update(cx, |view, cx| view.set_alternate(alternate, cx));
        visual.simulate_resize(size(px(width), px(660.0)));
        for entry in ENTRIES {
            choose(&mut visual, &view, entry);
            let viewport = visual.debug_bounds("sample-viewport").unwrap();
            assert!(viewport.origin.x >= px(0.0) && viewport.right() <= px(width));
            assert!(viewport.origin.y >= px(0.0) && viewport.bottom_left().y <= px(660.0));
            let card = visual.debug_bounds("catalog-card").unwrap();
            let header = visual.debug_bounds("device-frame-header").unwrap();
            let body = visual.debug_bounds("fixture-body").unwrap();
            assert_eq!(card.size, size(px(entry.width()), px(184.0)), "{entry:?}");
            assert_eq!(
                header.size.height,
                px(hex_widgets::DEVICE_FRAME_HEADER_HEIGHT)
            );
            assert_eq!(body.size.height, px(device_composition::BODY_HEIGHT));
            assert_eq!(body.origin.y, header.bottom_left().y);
            assert_eq!(
                visual.rendered_background("device-frame-header"),
                Some(gpui::Background::from(rgb(theme::device_style(
                    theme::DeviceStyleRole::Layout
                )
                .surface)))
            );
            let scale = visual.update(|window, _| window.scale_factor());
            let (_, mask) = visual
                .rendered_surface_geometry("catalog-card")
                .expect("painted production card");
            assert!(
                mask.right() <= viewport.scale(scale).right(),
                "wide cards stay clipped to the viewport"
            );
            for (toggle, panel) in [
                ("device-macro-toggle", "device-macro-panel"),
                ("device-modulator-toggle", "device-modulator-panel"),
            ] {
                let button = visual.debug_bounds(toggle).expect("shared housing toggle");
                assert!(header.contains(&button.center()));
                visual.simulate_click(button.center(), Modifiers::default());
                visual.run_until_parked();
                let bounds = visual
                    .debug_bounds(panel)
                    .expect("shared placeholder panel");
                assert!(card.contains(&bounds.center()));
                visual.simulate_click(button.center(), Modifiers::default());
                visual.run_until_parked();
                assert!(visual.debug_bounds(panel).is_none());
            }

            if entry.is_effect() {
                visual.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.owner = Owner::Track;
                        cx.notify();
                    })
                });
                visual.run_until_parked();
                let toggle = visual
                    .debug_bounds("owner-sample")
                    .expect("effect owner control");
                visual.simulate_click(toggle.center(), Modifiers::default());
                visual.run_until_parked();
                view.read_with(cx, |view, _| assert_eq!(view.owner, Owner::Return));
                assert_eq!(
                    visual.debug_bounds("catalog-card"),
                    Some(card),
                    "{entry:?} Return"
                );
                assert_eq!(
                    visual.debug_bounds("fixture-body"),
                    Some(body),
                    "{entry:?} Return body"
                );
            }
        }
    }
}

#[gpui::test]
fn selection_focus_hover_and_bypass_preserve_body_paint_and_real_control_footprints(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let body = visual.debug_bounds("fixture-body").unwrap();
    let paint = visual.rendered_background("fixture-body");
    let field = visual
        .debug_bounds(number_id(Entry::Utility, 0, "-value"))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.selected, "body/control clicks select the card")
    });
    assert_eq!(visual.debug_bounds("fixture-body"), Some(body));
    assert_eq!(visual.rendered_background("fixture-body"), paint);
    assert_eq!(
        visual.rendered_background("device-frame-header"),
        Some(gpui::Background::from(rgb(theme::device_style(
            theme::DeviceStyleRole::Layout
        )
        .accent)))
    );
    visual.simulate_keystrokes("escape");
    let bypass = visual.debug_bounds("bypass-sample").unwrap();
    visual.simulate_click(bypass.center(), Modifiers::default());
    view.read_with(cx, |view, _| assert!(view.bypassed));
    assert_eq!(
        visual.debug_bounds(number_id(Entry::Utility, 0, "-value")),
        Some(field)
    );
    assert_eq!(visual.rendered_background("fixture-body"), paint);
    let action = visual.debug_bounds("fixture-header-bypass").unwrap();
    assert_eq!(action.size, size(px(12.0), px(12.0)));
    visual.simulate_mouse_move(action.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("fixture-header-bypass"),
        None,
        "boolean hover must not restore the action box"
    );
    assert_eq!(
        visual.rendered_background("fixture-header-bypass-indicator"),
        Some(gpui::Background::from(
            gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Border))).opacity(0.8)
        ))
    );
}

#[gpui::test]
fn header_power_toggle_keeps_owner_state_when_disabled_and_has_a_full_hit_target(
    cx: &mut TestAppContext,
) {
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target = visual.debug_bounds("fixture-header-bypass").unwrap();
    let indicator_bounds = visual
        .debug_bounds("fixture-header-bypass-indicator")
        .unwrap();
    assert_eq!(target.size, size(px(12.0), px(12.0)));
    assert_eq!(indicator_bounds.size, size(px(8.0), px(8.0)));
    assert_eq!(indicator_bounds.center(), target.center());
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(
        visual
            .rendered_surface_geometry("fixture-header-bypass-indicator")
            .unwrap()
            .0
            .top_left,
        px(hex_widgets::TOGGLE_CORNER_RADIUS).scale(scale)
    );
    assert_eq!(visual.rendered_background("fixture-header-bypass"), None);
    let on = visual.rendered_background("fixture-header-bypass-indicator");
    visual.simulate_mouse_move(target.center(), None, Modifiers::default());
    visual.simulate_mouse_down(target.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_ne!(
        visual.rendered_background("fixture-header-bypass-indicator"),
        on
    );
    visual.simulate_mouse_up(target.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.bypassed));
    let off = visual.rendered_background("fixture-header-bypass-indicator");
    assert_ne!(on, off);
    view.update(cx, |v, cx| {
        v.disabled = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(target.center(), Modifiers::default());
    view.read_with(cx, |v, _| assert!(v.bypassed));
    assert_eq!(
        visual.rendered_background("fixture-header-bypass-indicator"),
        off
    );
    view.update(cx, |v, cx| {
        v.disabled = false;
        cx.notify();
    });
    visual.run_until_parked();
    // The transparent corner outside the dot remains part of the hit target.
    visual.simulate_click(
        target.origin + gpui::point(px(0.5), px(0.5)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    view.read_with(cx, |v, _| assert!(!v.bypassed));
    visual.simulate_mouse_move(
        gpui::point(px(500.0), px(500.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("fixture-header-bypass-indicator"),
        on
    );
}

#[gpui::test]
fn long_active_numeric_draft_keeps_entity_focus_bounds_and_text_across_theme_selection(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let number = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Utility].numbers[0].entity.clone()
    });
    let field = visual
        .debug_bounds(number_id(Entry::Utility, 0, "-value"))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    let text = "123456789012345678901234567890 invalid fixture value";
    visual.simulate_input(text);
    let focus = cx.update(|app| number.focus_handle(app));
    view.update(cx, |view, cx| {
        view.selected = true;
        view.set_alternate(true, cx);
    });
    visual.run_until_parked();
    view.read_with(cx, |view, app| {
        assert_eq!(
            view.fixtures[&Entry::Utility].numbers[0].entity.entity_id(),
            number.entity_id()
        );
        assert!(number.read(app).is_editing());
        assert_eq!(number.read(app).display_text(), text);
    });
    assert!(visual.update(|window, _| focus.is_focused(window)));
    assert_eq!(
        visual.debug_bounds(number_id(Entry::Utility, 0, "-value")),
        Some(field)
    );
    assert_eq!(
        visual.rendered_background(number_id(Entry::Utility, 0, "-value")),
        Some(gpui::Background::from(rgb(theme::color(
            theme::PaletteRole::Background
        ))))
    );
    visual.simulate_keystrokes("tab");
    assert!(
        visual.update(|window, _| focus.is_focused(window)),
        "invalid draft refuses traversal"
    );
    visual.simulate_keystrokes("escape");
}

#[gpui::test]
fn selector_popup_is_real_retained_foreground_and_theme_refresh_keeps_focus(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Saturator);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let choice = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Saturator].selectors[0].clone()
    });
    let field_id = "fixture-saturator-selector-0";
    let field = visual.debug_bounds(field_id).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(choice.read_with(cx, |control, _| control.is_open()));
    let popup = visual
        .debug_bounds("fixture-saturator-selector-0-popup")
        .unwrap();
    let card = visual.debug_bounds("catalog-card").unwrap();
    assert!(
        popup.bottom() > card.bottom() || popup.top() < card.top(),
        "popup escapes the clipped device body: popup {popup:?}, card {card:?}"
    );
    let focus = cx.update(|app| choice.focus_handle(app));
    view.update(cx, |view, cx| view.set_alternate(true, cx));
    visual.run_until_parked();
    assert!(choice.read_with(cx, |control, _| control.is_open()));
    assert!(visual.update(|window, _| focus.is_focused(window)));
    assert_eq!(visual.debug_bounds(field_id), Some(field));
    assert_eq!(
        visual.rendered_background("fixture-saturator-selector-0-popup"),
        Some(gpui::Background::from(rgb(theme::device_style(
            theme::DeviceStyleRole::Control
        )
        .surface)))
    );
    visual.simulate_keystrokes("down");
    assert_eq!(
        choice.read_with(cx, |control, _| control.selected()),
        1,
        "real widget choice changes locally"
    );
    visual.simulate_keystrokes("enter escape");
    assert!(!choice.read_with(cx, |control, _| control.is_open()));
}

#[gpui::test]
fn disabled_controls_refuse_input_without_losing_selection_geometry_or_local_state(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let toggle = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Utility].toggles[0].clone()
    });
    let bounds = visual.debug_bounds("fixture-utility-toggle-0").unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(!toggle.read_with(cx, |control, _| control.checked()));
    let card = visual.debug_bounds("catalog-card").unwrap();
    view.update(cx, |view, cx| {
        view.disabled = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(!toggle.read_with(cx, |control, _| control.checked()));
    assert_eq!(visual.debug_bounds("catalog-card"), Some(card));
    assert_eq!(
        visual.debug_bounds("fixture-utility-toggle-0"),
        Some(bounds)
    );
}

#[gpui::test]
fn production_vector_tabs_keep_one_page_and_stable_graph_slots_through_mixed_input(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Vector);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let tab = visual.debug_bounds("oscillator").unwrap();
    let graph = visual.debug_bounds("fixture-vector-graph").unwrap();
    assert_eq!(graph.size.height, px(86.0));
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |view, _| assert_eq!(view.active_tab, 1));
    visual.simulate_keystrokes("left enter");
    visual.run_until_parked();
    view.read_with(cx, |view, _| assert_eq!(view.active_tab, 0));
    view.update(cx, |view, cx| {
        view.selected = true;
        view.set_alternate(true, cx);
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("oscillator"), Some(tab));
    assert_eq!(visual.debug_bounds("fixture-vector-graph"), Some(graph));
}

#[gpui::test]
fn vector_section_focus_is_independent_and_invalid_drafts_refuse_tab_without_resetting_header(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Vector);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let number = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Vector].numbers[0].entity.clone()
    });
    visual.update(|window, app| number.focus_handle(app).focus(window));
    visual.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.section, Some(0));
        assert!(
            !view.selected,
            "section keyboard focus does not select the device"
        );
    });
    assert_eq!(
        visual.rendered_background("fixture-shell-row-0"),
        Some(gpui::Background::from(rgb(theme::device_style(
            theme::DeviceStyleRole::Layout
        )
        .surface)))
    );
    view.update(cx, |view, cx| {
        view.selected = true;
        cx.notify();
    });
    let field = visual
        .debug_bounds(number_id(Entry::Vector, 0, "-value"))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid section draft");
    let focus = cx.update(|app| number.focus_handle(app));
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    view.read_with(cx, |view, app| {
        assert_eq!(view.section, Some(0));
        assert!(view.selected);
        assert!(number.read(app).is_editing());
        assert_eq!(number.read(app).display_text(), "invalid section draft");
    });
    assert!(visual.update(|window, _| focus.is_focused(window)));
    visual.simulate_keystrokes("escape");
    let next = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Vector].numbers[13].entity.clone()
    });
    visual.update(|window, app| next.focus_handle(app).focus(window));
    visual.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.section, Some(4));
        assert!(view.selected);
    });
    assert_eq!(
        visual.rendered_background("fixture-preview-row-0"),
        Some(gpui::Background::from(rgb(theme::device_style(
            theme::DeviceStyleRole::Layout
        )
        .surface)))
    );
    assert_eq!(
        visual.rendered_background("fixture-shell-row-0"),
        Some(gpui::Background::from(rgb(theme::device_style(
            theme::DeviceStyleRole::Layout
        )
        .rail)))
    );
}

#[gpui::test]
fn eq_catalog_uses_shared_live_geometry_controls_and_registered_response_without_audio(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let window = cx.add_window(|_, cx| {
        let mut view = Gallery::new(cx);
        view.entry = Entry::Eq;
        view.ensure_fixture(cx);
        view
    });
    let handle = *window;
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    for width in [760.0, 1040.0, 1320.0] {
        visual.simulate_resize(size(px(width), px(720.0)));
        visual.run_until_parked();
        let body = visual.debug_bounds("fixture-body").unwrap();
        let graph = visual.debug_bounds("fixture-eq-graph").unwrap();
        assert!(
            graph.size.width >= body.size.width * 0.7
                && graph.size.height >= body.size.height * 0.9
        );
    }
    let q_control = view.read_with(cx, |view, _| {
        view.fixtures[&Entry::Eq].numbers[3].entity.clone()
    });
    q_control.update(cx, |q, cx| q.set_external_value(18.0, cx).unwrap());
    let before = view.read_with(cx, |view, cx| {
        eq::model(&view.fixtures[&Entry::Eq], cx).unwrap()
    });
    before.validate().unwrap();
    let center = before.nodes[0].position[0];
    assert!(
        before
            .primary
            .iter()
            .any(|point| (point[0] - center).abs() < 1.0e-6),
        "the narrow node center must be sampled"
    );
    assert!(before.analyzer.is_some());
    assert!(before.input_analyzer.is_some());
    assert!(before.analyzer_peak.is_some());
    let field = visual.debug_bounds("fixture-eq-numeric-2-value").unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-12");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let after = view.read_with(cx, |view, cx| {
        eq::model(&view.fixtures[&Entry::Eq], cx).unwrap()
    });
    after.validate().unwrap();
    assert_ne!(
        after.primary, before.primary,
        "real registered response follows the shared numeric control"
    );
    assert_eq!(after.analyzer, before.analyzer);
    assert_eq!(after.input_analyzer, before.input_analyzer);
    assert_eq!(after.analyzer_peak, before.analyzer_peak);
    let choice = visual.debug_bounds("fixture-eq-selector-1").unwrap();
    visual.simulate_click(choice.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("fixture-eq-selector-1-popup").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("fixture-eq-selector-1-popup").is_none());
}

#[gpui::test]
fn shared_housing_visibility_does_not_transfer_to_another_fixture(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx, Entry::Utility);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(760.0), px(660.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-macro-toggle").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-macro-panel").is_some());
    choose(&mut visual, &view, Entry::Saturator);
    assert!(visual.debug_bounds("device-macro-panel").is_none());
    assert!(visual.debug_bounds("device-modulator-panel").is_none());
}
