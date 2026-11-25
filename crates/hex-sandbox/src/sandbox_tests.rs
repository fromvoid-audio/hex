use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
struct RestoreTheme(hex_preferences::ThemePreferences);
impl Drop for RestoreTheme {
    fn drop(&mut self) {
        theme::apply_preferences(self.0);
    }
}
fn open(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Entity<Sandbox>) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    cx.update(hex_widgets::bind_response_graph_keys);
    theme::apply_preferences(hex_preferences::ThemePreferences::default());
    let handle = cx.add_window(|_, cx| Sandbox::new(cx));
    (*handle, handle.root(cx).expect("sandbox"))
}
fn choose(visual: &mut VisualTestContext, view: &Entity<Sandbox>, name: &str) {
    visual.update(|window, cx| {
        view.update(cx, |s, cx| {
            let i = s
                .items
                .iter()
                .position(|i| i.name == name)
                .expect("named inventory entry");
            s.choose(i, window, cx);
        })
    });
    visual.run_until_parked();
}
// GPUI's test selector API requires a static key; these small fixture-only
// strings live for the duration of the test process.
fn key(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
fn selector(view: &Entity<Sandbox>, cx: &TestAppContext, suffix: &str) -> &'static str {
    key(view.read_with(cx, |s, _| format!("sample-{}-{suffix}", s.selected)))
}
#[gpui::test]
fn navigation_and_fixture_toggles_keep_small_corners_across_selection_and_theme(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1040.0), px(720.0)));
    for _ in 0..2 {
        visual.run_until_parked();
        let scale = visual.update(|window, _| window.scale_factor());
        for category in ["discrete", "numeric"] {
            let target = visual.debug_bounds(category).expect("category");
            visual.simulate_click(target.center(), Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |s, _| assert_eq!(s.category, category));
            let pages = view.read_with(cx, |s, _| {
                s.pages
                    .iter()
                    .enumerate()
                    .filter(|(_, page)| page.category == category)
                    .map(|(index, _)| key(format!("page-{index}")))
                    .collect::<Vec<_>>()
            });
            for page in &pages {
                let target = visual.debug_bounds(page).expect("page");
                visual.simulate_click(target.center(), Modifiers::default());
                visual.run_until_parked();
                for selector in ["numeric", "discrete", "sandbox-theme"]
                    .into_iter()
                    .chain(pages.iter().copied())
                    .chain([
                        selector(&view, &visual, "enable"),
                        selector(&view, &visual, "width"),
                    ])
                {
                    assert_eq!(
                        visual
                            .rendered_surface_geometry(selector)
                            .expect("toggle surface")
                            .0
                            .top_left,
                        px(hex_widgets::TOGGLE_CORNER_RADIUS).scale(scale),
                        "{selector}"
                    );
                }
            }
        }
        view.update(&mut visual, |s, cx| s.theme(cx));
    }
}

#[gpui::test]
fn every_named_entry_renders_in_bounded_native_viewports(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let pages = view.read_with(cx, |s, _| s.pages.clone());
    for (width, light) in [(1040.0, false), (760.0, true)] {
        if light {
            view.update(cx, |s, cx| s.theme(cx));
        }
        visual.simulate_resize(size(px(width), px(720.0)));
        for page in &pages {
            let name = view.read_with(cx, |s, _| {
                s.items
                    .iter()
                    .find(|item| item.page() == *page)
                    .unwrap()
                    .name
            });
            choose(&mut visual, &view, name);
            view.update(cx, |s, cx| {
                for index in s.configurations() {
                    if let Some(sample) = s.samples.get(&index) {
                        sample.update(cx, |sample, cx| {
                            sample.narrow = light;
                            cx.notify();
                        });
                    }
                }
            });
            visual.run_until_parked();
            view.read_with(cx, |s, cx| {
                assert!(s.error.is_none(), "{name}: {:?}", s.error);
                for index in s.configurations() {
                    if let Some(sample) = s.samples.get(&index) {
                        assert!(!sample.read(cx).status.contains("invalid"), "{name}");
                    }
                }
            });
            for index in view.read_with(cx, |s, _| s.configurations()) {
                assert!(
                    visual
                        .debug_bounds(key(format!("configuration-{index}")))
                        .is_some()
                );
            }
            let content = visual.debug_bounds("sandbox-content").expect("viewport");
            assert!(
                content.origin.x >= px(0.0) && content.right() <= px(width),
                "{name}"
            );
            assert!(content.size.height > px(200.0), "{name}");
        }
    }
}
#[gpui::test]
fn controls_emit_local_feedback_and_retain_values_across_navigation_theme_and_disable(
    cx: &mut TestAppContext,
) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    choose(&mut visual, &view, "standard toggle");
    let hit = visual
        .debug_bounds(selector(&view, cx, "sandbox-toggle"))
        .expect("toggle");
    visual.simulate_click(hit.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert_eq!(s.samples[&s.selected].read(cx).status, "toggle · false")
    });
    choose(&mut visual, &view, "standard knob");
    visual.update(|window, cx| {
        view.update(cx, |s, cx| {
            s.samples[&s.selected].update(cx, |s, cx| s.focus_first(window, cx))
        })
    });
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert!(
            s.samples[&s.selected]
                .read(cx)
                .status
                .starts_with("value 1")
        )
    });
    choose(&mut visual, &view, "standard toggle");
    let enable = visual
        .debug_bounds(selector(&view, cx, "enable"))
        .expect("disable control");
    visual.simulate_click(enable.center(), Modifiers::default());
    visual.run_until_parked();
    let hit = visual
        .debug_bounds(selector(&view, cx, "sandbox-toggle"))
        .expect("toggle");
    visual.simulate_click(hit.center(), Modifiers::default());
    visual.run_until_parked();
    view.update(cx, |s, cx| s.theme(cx));
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        let sample = s.samples[&s.selected].read(cx);
        assert!(!sample.enabled);
        assert_eq!(sample.status, "toggle · false");
    });
    choose(&mut visual, &view, "standard selector");
    let hit = visual
        .debug_bounds(selector(&view, cx, "sandbox-selector"))
        .expect("selector");
    visual.simulate_click(hit.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(selector(&view, cx, "sandbox-selector-option-1"))
        .expect("real popup");
    visual.simulate_click(option.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert_eq!(
            s.samples[&s.selected].read(cx).status,
            "choice 2 · triangle"
        )
    });
}
#[gpui::test]
fn bars_graph_color_and_chrome_accept_real_pointer_input(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (name, selector, prefix) in [
        ("bar editor", "sandbox-bars", "bar ·"),
        ("response graph", "sandbox-graph", "node "),
        ("color picker", "sandbox-color-field", "color ·"),
        ("action", "chrome-hit", "local action ·"),
    ] {
        choose(&mut visual, &view, name);
        let hit = visual
            .debug_bounds(self::selector(&view, cx, selector))
            .expect(name);
        if name == "action" {
            let action = self::selector(&view, cx, "chrome-action");
            let bounds = visual.debug_bounds(action).unwrap();
            visual.simulate_mouse_move(bounds.center(), None, Modifiers::default());
            visual.run_until_parked();
            let hover = visual.rendered_background(action);
            visual.simulate_mouse_down(
                bounds.center(),
                gpui::MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_background(action),
                Some(rgb(theme::action_press_style(theme::action_style()).background).into())
            );
            assert_ne!(visual.rendered_background(action), hover);
            visual.simulate_mouse_up(
                bounds.center(),
                gpui::MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            assert_eq!(visual.rendered_background(action), hover);
        } else {
            visual.simulate_click(hit.center(), Modifiers::default());
            visual.run_until_parked();
        }
        view.read_with(cx, |s, cx| {
            assert!(
                s.samples[&s.selected].read(cx).status.starts_with(prefix),
                "{name}: {}",
                s.samples[&s.selected].read(cx).status
            )
        });
    }
}

#[gpui::test]
fn configurations_coexist_with_independent_state_and_scoped_reset(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    choose(&mut visual, &view, "standard toggle");
    let names = ["standard toggle", "compact button"];
    let indices = view.read_with(cx, |s, _| {
        names.map(|name| s.items.iter().position(|item| item.name == name).unwrap())
    });
    let hit = visual
        .debug_bounds(key(format!("sample-{}-sandbox-toggle", indices[0])))
        .unwrap();
    visual.simulate_click(hit.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert_eq!(s.configurations().len(), 5);
        assert_eq!(s.samples[&indices[0]].read(cx).status, "toggle · false");
        assert_eq!(
            s.samples[&indices[1]].read(cx).status,
            "local fixture · ready"
        );
    });
    for index in view.read_with(cx, |s, _| s.configurations()) {
        assert!(
            visual
                .debug_bounds(key(format!("sample-{index}-widget-preview")))
                .is_some()
        );
    }
    choose(&mut visual, &view, "compact button");
    let enable = visual
        .debug_bounds(key(format!("sample-{}-enable", indices[1])))
        .unwrap();
    visual.simulate_click(enable.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert!(s.samples[&indices[0]].read(cx).enabled);
        assert!(!s.samples[&indices[1]].read(cx).enabled);
    });
    let reset = visual
        .debug_bounds(key(format!("sample-{}-reset", indices[1])))
        .unwrap();
    visual.simulate_click(reset.center(), Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |s, cx| {
        assert_eq!(s.samples[&indices[0]].read(cx).status, "toggle · false");
        assert!(s.samples[&indices[1]].read(cx).enabled);
    });
}

#[gpui::test]
fn numeric_kind_pages_navigate_without_mixing_configurations(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (name, count) in [
        ("knob", 7),
        ("horizontal slider", 5),
        ("vertical slider", 1),
    ] {
        let page_index = view.read_with(cx, |s, _| {
            s.pages.iter().position(|page| page.name == name).unwrap()
        });
        let hit = visual
            .debug_bounds(key(format!("page-{page_index}")))
            .unwrap();
        visual.simulate_click(hit.center(), Modifiers::default());
        visual.run_until_parked();
        let membership = view.read_with(cx, |s, _| {
            assert_eq!(s.page().name, name);
            assert_eq!(s.configurations().len(), count);
            s.items
                .iter()
                .enumerate()
                .filter(|(_, item)| matches!(item.demo, Demo::Numeric(..)))
                .map(|(index, item)| (index, item.name, item.page() == s.page()))
                .collect::<Vec<_>>()
        });
        for (index, item_name, visible) in membership {
            assert_eq!(
                visual
                    .debug_bounds(key(format!("configuration-{index}")))
                    .is_some(),
                visible,
                "{item_name} on {name}"
            );
        }
    }
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    view.read_with(cx, |s, _| assert_eq!(s.page().name, "horizontal slider"));
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    view.read_with(cx, |s, _| assert_eq!(s.page().name, "knob"));
}
