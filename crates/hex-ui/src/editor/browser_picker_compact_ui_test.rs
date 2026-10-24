//! Pointer resize and painted compact-heading contract; no native GPU claim.
use super::*;

fn picker(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    PreferencesTestDirectory,
) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = PreferencesTestDirectory::new();
    let samples = directory.0.join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(
                    window,
                    cx,
                    Service::new(48_000, 16, 8_000_000).unwrap(),
                    &directory.0,
                )
            })
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.browser.directory, samples);
        assert_eq!(view.browser.places, vec![samples.clone()]);
        assert_eq!(view.sample_roots(), vec![samples]);
    });
    (handle, editor, directory)
}

#[gpui::test]
fn browser_picker_near_minimum_resize_captures_and_retains_exact_control_minimum(
    cx: &mut TestAppContext,
) {
    let (handle, _editor, _directory) = picker(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let minimum = f32::from(visual.debug_bounds("browser-control-⚙").unwrap().size.width) + 1.0;
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let before = visual.debug_bounds("browser-sidebar").unwrap();
    let near = minimum + f32::from(divider.size.width) * 0.5;
    let target = divider.center() + point(px(near) - before.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(minimum),
        "near-minimum drag must capture the exact existing minimum and stay there after release"
    );
}

fn resize(visual: &mut VisualTestContext, width: f32) {
    let before = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let target = divider.center() + point(px(width) - before.size.width, px(0.0));
    visual.simulate_mouse_down(divider.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
fn selector(prefix: &str, name: &str) -> &'static str {
    Box::leak(format!("browser-picker-heading-{prefix}{name}").into_boxed_str())
}

#[gpui::test]
fn browser_picker_compact_heading_paint_retains_every_source_row_and_settings_slot(
    cx: &mut TestAppContext,
) {
    let (handle, editor, directory) = picker(cx);
    let long_place = directory
        .0
        .join("samples")
        .join("a very long custom place name for clipping");
    std::fs::create_dir(&long_place).unwrap();
    editor.update(cx, |view, cx| {
        view.browser.places[0] = long_place.clone();
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let rows = [
        "browser-collection-0",
        "browser-collection-1",
        "browser-collection-2",
        "browser-section-samples",
        "browser-section-devices",
        "browser-section-projects",
        "browser-section-all",
        "browser-section-vst3",
        "browser-control-⚙",
        "browser-place-0",
    ];
    let glyphs = [
        "browser-picker-glyph-collection-0",
        "browser-picker-glyph-collection-1",
        "browser-picker-glyph-collection-2",
        "browser-picker-glyph-section-0",
        "browser-picker-glyph-section-1",
        "browser-picker-glyph-section-2",
        "browser-picker-glyph-section-3",
        "browser-picker-glyph-section-4",
        "browser-picker-glyph-place-0",
    ];
    let outer = visual.debug_bounds("browser-resize-handle").unwrap();
    for settings in [false, true] {
        resize(visual, 124.0);
        if settings {
            let gear = visual.debug_bounds("browser-control-⚙").unwrap();
            visual.simulate_click(gear.center(), Modifiers::default());
            visual.run_until_parked();
        }
        let before_rows = rows.map(|name| visual.debug_bounds(name).unwrap());
        let before_glyphs = glyphs.map(|name| visual.debug_bounds(name).unwrap());
        let names = if settings {
            vec![
                "collections",
                "collections pending",
                "library",
                "sections · session",
                "places",
            ]
        } else {
            vec!["collections", "collections pending", "library", "places"]
        };
        let headings = names
            .iter()
            .map(|name| (*name, visual.debug_bounds(selector("", name)).unwrap()))
            .collect::<Vec<_>>();
        for (requested, expected) in [
            (124.0, 124.0),
            (80.0, 80.0),
            (36.0, 36.0),
            (25.0, 23.0),
            (28.0, 23.0),
            (35.0, 35.0),
            (23.0, 23.0),
            (124.0, 124.0),
        ] {
            resize(visual, requested);
            assert_eq!(
                visual.debug_bounds("browser-sidebar").unwrap().size.width,
                px(expected)
            );
            assert_eq!(visual.debug_bounds("browser-resize-handle").unwrap(), outer);
            for (name, before) in rows.into_iter().zip(before_rows) {
                let after = visual.debug_bounds(name).unwrap();
                assert_eq!(after.origin, before.origin, "row {name} at {requested}");
                assert_eq!(
                    after.size.height, before.size.height,
                    "row {name} at {requested}"
                );
            }
            for (name, before) in glyphs.into_iter().zip(before_glyphs) {
                assert_eq!(
                    visual.debug_bounds(name).unwrap(),
                    before,
                    "glyph {name} at {requested}"
                );
            }
            for (name, before) in &headings {
                let after = visual.debug_bounds(selector("", name)).unwrap();
                assert_eq!(after.origin, before.origin);
                assert_eq!(after.size.height, before.size.height);
                let text = visual.debug_bounds(selector("text-", name));
                let mark = visual.debug_bounds(selector("mark-", name));
                if expected == 23.0 {
                    assert!(
                        text.is_none(),
                        "hidden heading descendants must not reach paint: {name}"
                    );
                    let mark = mark.expect("compact separator paints");
                    assert_eq!(
                        mark.center().x,
                        visual.debug_bounds("browser-control-⚙").unwrap().center().x
                    );
                    assert_eq!(mark.size.height, px(1.0));
                    assert!(mark.origin.y >= after.origin.y && mark.bottom() <= after.bottom());
                    assert!(
                        visual
                            .rendered_background(selector("mark-", name))
                            .is_some(),
                        "actual separator scene quad"
                    );
                } else {
                    assert!(text.is_some(), "expanded heading text paints: {name}");
                    assert!(mark.is_none());
                }
            }
            if expected == 23.0 {
                assert_eq!(
                    visual
                        .debug_bounds("browser-picker-label-place-0")
                        .unwrap()
                        .size
                        .width,
                    px(0.0)
                );
            }
        }
    }
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.places[0], long_place);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn browser_picker_compact_drag_hysteresis_avoids_chatter_and_keeps_other_widths_continuous(
    cx: &mut TestAppContext,
) {
    let (handle, _editor, _directory) = picker(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("browser-sidebar").unwrap();
    let divider = visual.debug_bounds("browser-sidebar-resize").unwrap();
    let start = divider.center();
    let minimum = f32::from(visual.debug_bounds("browser-control-⚙").unwrap().size.width) + 1.0;
    let band = f32::from(divider.size.width);
    let cases = [
        (minimum + band + 1.0, minimum + band + 1.0),
        (minimum + band * 0.5, minimum),
        (minimum + band + 1.0, minimum),
        (minimum + 2.0 * band, minimum),
        (minimum + 2.0 * band + 1.0, minimum + 2.0 * band + 1.0),
        (minimum + band + 1.0, minimum + band + 1.0),
        (minimum + band, minimum),
        (minimum + band + 0.5, minimum),
        (minimum - 10.0, minimum),
        (80.5, 80.5),
        (79.25, 79.25),
    ];
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for (raw, expected) in cases {
        let target = start + point(px(raw) - sidebar.size.width, px(0.0));
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let painted = visual.debug_bounds("browser-sidebar").unwrap().size.width;
        if expected.fract() == 0.0 {
            assert_eq!(painted, px(expected), "raw width {raw}");
        } else {
            // GPUI rounds fractional line/box origins to the device-pixel grid.
            // Capture remains exact; ordinary widths retain only this paint rounding.
            assert!(
                f32::from(painted - px(expected)).abs() <= 0.5,
                "raw width {raw}: {painted:?}"
            );
        }
    }
    let target = start + point(px(minimum + band * 0.5) - sidebar.size.width, px(0.0));
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("browser-sidebar").unwrap().size.width,
        px(minimum)
    );
    for _ in 0..3 {
        resize(visual, minimum + band);
        assert_eq!(
            visual.debug_bounds("browser-sidebar").unwrap().size.width,
            px(minimum)
        );
        resize(visual, 90.0);
        assert_eq!(
            visual.debug_bounds("browser-sidebar").unwrap().size.width,
            px(90.0)
        );
        resize(visual, minimum + band * 0.5);
    }
}
