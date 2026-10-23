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
#[gpui::test]
fn every_named_entry_renders_in_bounded_native_viewports(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let (handle, view) = open(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let names = view.read_with(cx, |s, _| {
        s.items.iter().map(|i| i.name).collect::<Vec<_>>()
    });
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
    for (width, light) in [(1040.0, false), (760.0, true)] {
        if light {
            view.update(cx, |s, cx| s.theme(cx));
        }
        visual.simulate_resize(size(px(width), px(720.0)));
        for name in &names {
            choose(&mut visual, &view, name);
            view.read_with(cx, |s, cx| {
                assert!(s.error.is_none(), "{name}: {:?}", s.error);
                if let Some(sample) = s.samples.get(&s.selected) {
                    assert!(!sample.read(cx).status.contains("invalid"), "{name}");
                }
            });
            let content = visual.debug_bounds("sandbox-content").expect("viewport");
            assert!(
                content.origin.x >= px(0.0) && content.right() <= px(width),
                "{name}"
            );
            assert!(content.size.height > px(200.0), "{name}");
            if *name != "device compositions · 12 fixtures" {
                assert!(visual.debug_bounds("widget-preview").is_some(), "{name}");
            }
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
    let hit = visual.debug_bounds("sandbox-toggle").expect("toggle");
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
        .debug_bounds("sandbox-enable")
        .expect("disable control");
    visual.simulate_click(enable.center(), Modifiers::default());
    visual.run_until_parked();
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
    let hit = visual.debug_bounds("sandbox-selector").expect("selector");
    visual.simulate_click(hit.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds("sandbox-selector-option-1")
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
        let hit = visual.debug_bounds(selector).expect(name);
        visual.simulate_click(hit.center(), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, cx| {
            assert!(
                s.samples[&s.selected].read(cx).status.starts_with(prefix),
                "{name}: {}",
                s.samples[&s.selected].read(cx).status
            )
        });
    }
}
