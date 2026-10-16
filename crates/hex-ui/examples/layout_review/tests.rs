use super::*;
use gpui::{Focusable, Modifiers, TestAppContext, VisualTestContext, size};

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
        Some(gpui::Background::from(rgb(theme::color(
            theme::PaletteRole::Hover
        ))))
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
    assert!(
        popup.bottom_left().y > visual.debug_bounds("catalog-card").unwrap().bottom_left().y,
        "popup escapes the clipped device body"
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
