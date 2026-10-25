use super::*;
use gpui::{Context, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext};

#[test]
fn invalid_and_unbounded_rule_inputs_are_refused() {
    let rule = VerticalRule::new(0.5, px(1.5), 0xabcdef, 0.42).unwrap();
    assert!(vertical_rule_layer([]).is_none());
    assert!(vertical_rule_layer([rule]).is_some());
    assert!(vertical_rule_layer(std::iter::repeat(rule)).is_none());
    assert!(vertical_rule_layer(std::iter::repeat_n(rule, MAX_RULE_LAYER_RULES)).is_some());
    for position in [-0.1, 1.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(VerticalRule::new(position, px(1.0), 0, 0.42).is_none());
    }
    for thickness in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(VerticalRule::new(0.5, px(thickness), 0, 0.42).is_none());
    }
    for opacity in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
        assert!(VerticalRule::new(0.5, px(1.0), 0, opacity).is_none());
    }
}

// Retained original Div-tree strategy. It establishes native percentage/pixel
// rounding and opacity independently of the batched Element implementation.
fn legacy(rules: &[VerticalRule]) -> Div {
    div()
        .absolute()
        .inset_0()
        .children(rules.iter().map(|rule| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(rule.position))
                .w(rule.thickness)
                .bg(rgb(rule.color))
                .opacity(rule.opacity)
        }))
}

#[derive(Clone, Copy)]
struct Geometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    border: bool,
    opacity: f32,
}
struct Fixture {
    old: bool,
    geometry: Geometry,
    rules: Vec<VerticalRule>,
    clicks: usize,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let g = self.geometry;
        let layer = if self.old {
            Some(legacy(&self.rules))
        } else {
            vertical_rule_layer(self.rules.iter().copied())
        };
        div().size_full().relative().child(
            div()
                .absolute()
                .left(px(g.left))
                .top(px(g.top))
                .w(px(g.width))
                .h(px(g.height))
                .overflow_hidden()
                .opacity(g.opacity)
                .when(g.border, |host| host.border_1().border_color(rgb(0x445566)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _, _, cx| {
                        view.clicks += 1;
                        cx.notify();
                    }),
                )
                .children(layer.map(|layer| layer.debug_selector(|| "rule-fixture".into())))
                // A subsequent opaque overlay must retain its paint position.
                .child(
                    div()
                        .absolute()
                        .left(px(9.25))
                        .top(px(7.5))
                        .w(px(5.5))
                        .h(px(8.25))
                        .bg(rgb(0x345678)),
                ),
        )
    }
}

#[gpui::test]
fn batched_rules_match_native_legacy_quads_and_keep_parent_input(cx: &mut TestAppContext) {
    let rules = [
        (0.0, 1.0, 0x223344, 0.24),
        (0.125, 1.5, 0x88ccdd, 0.42),
        (0.33333, 0.5, 0x223344, 0.24),
        (0.5, 2.75, 0x88ccdd, 1.0),
        (0.999, 1.5, 0x223344, 0.42),
        (1.0, 1.0, 0x223344, 0.24),
    ]
    .into_iter()
    .map(|(position, width, color, opacity)| {
        VerticalRule::new(position, px(width), color, opacity).unwrap()
    })
    .collect::<Vec<_>>();
    let handle = cx.add_window(|_, _| Fixture {
        old: true,
        geometry: Geometry {
            left: 10.125,
            top: 20.375,
            width: 371.5,
            height: 37.25,
            border: false,
            opacity: 1.0,
        },
        rules,
        clicks: 0,
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    for geometry in [
        Geometry {
            left: 10.125,
            top: 20.375,
            width: 371.5,
            height: 37.25,
            border: false,
            opacity: 1.0,
        },
        Geometry {
            left: 7.325,
            top: -2.25,
            width: 93.125,
            height: 41.75,
            border: true,
            opacity: 0.63,
        },
        Geometry {
            left: -0.75,
            top: 8.25,
            width: 3.25,
            height: 22.5,
            border: true,
            opacity: 0.37,
        },
    ] {
        view.update(cx, |view, cx| {
            view.geometry = geometry;
            view.old = true;
            cx.notify();
        });
        visual.run_until_parked();
        let before_bounds = visual.debug_bounds("rule-fixture").unwrap();
        let before = visual.rendered_filled_quads("rule-fixture");
        assert!(!before.is_empty(), "reference must paint visible rules");
        view.update(cx, |view, cx| {
            view.old = false;
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("rule-fixture"), Some(before_bounds));
        assert_eq!(
            visual.rendered_filled_quads("rule-fixture"),
            before,
            "native bounds/color/opacity/clipping/order changed"
        );
        // Completed cache replay must preserve the same painter output too.
        view.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.rendered_filled_quads("rule-fixture"), before);
    }
    view.update(cx, |view, cx| {
        view.geometry = Geometry {
            left: 10.125,
            top: 20.375,
            width: 371.5,
            height: 37.25,
            border: false,
            opacity: 1.0,
        };
        cx.notify();
    });
    visual.run_until_parked();
    let root = visual.debug_bounds("rule-fixture").unwrap();
    visual.simulate_mouse_down(root.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(root.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.clicks),
        1,
        "passive rules intercepted parent input"
    );
}
