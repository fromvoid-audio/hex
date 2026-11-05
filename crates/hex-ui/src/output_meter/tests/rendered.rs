use super::*;
use gpui::{Context, Entity, Modifiers, Render, TestAppContext, VisualTestContext, Window, size};

struct Fixture {
    input: MeterInput,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let control = super::super::render(self.input);
        div()
            .size_full()
            .font_family(theme::FONT_FAMILY)
            .flex()
            .flex_col()
            .child(div().flex_1())
            // Reproduce the status-slot budget without touching editor wiring.
            .child(
                div()
                    .debug_selector(|| "meter-status-fixture".into())
                    .h(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .child(
                        div()
                            .debug_selector(|| "meter-message-fixture".into())
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(div().w(px(176.0)).flex_shrink_0())
                    .child(div().w(px(104.0)).flex_shrink_0())
                    .child(div().w(px(54.0)).flex_shrink_0())
                    .child(control)
                    .child(
                        div()
                            .debug_selector(|| "meter-diagnostics-fixture".into())
                            .w(px(168.0))
                            .flex_shrink_0(),
                    ),
            )
    }
}

fn change(fixture: &Entity<Fixture>, visual: &mut VisualTestContext, input: MeterInput) {
    fixture.update(&mut visual.cx, |fixture, cx| {
        fixture.input = input;
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn output_meter_uses_shared_level_colors_and_click_clear_clipping_caps(cx: &mut TestAppContext) {
    let _restore = RestoreTheme(theme::preferences());
    let window = cx.add_window(|_, _| Fixture {
        input: MeterInput::Offline,
    });
    let fixture = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    for preferences in [
        hex_preferences::ThemePreferences::default(),
        hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(230, 230, 230),
            text: hex_preferences::RgbColor::new(20, 20, 20),
            primary: hex_preferences::RgbColor::new(180, 110, 20),
            secondary: hex_preferences::RgbColor::new(20, 120, 70),
        },
    ] {
        theme::apply_preferences(preferences);
        let signal = theme::color(theme::PaletteRole::Secondary);
        let warning = theme::color(theme::PaletteRole::Primary);
        for (amplitude, color, above) in [
            (0.1, signal, false),
            (
                f32::from_bits(hex_widgets::METER_NEAR_PEAK_AMPLITUDE.to_bits() - 1),
                signal,
                false,
            ),
            (hex_widgets::METER_NEAR_PEAK_AMPLITUDE, warning, false),
            (0.75, signal, false),
            (1.0, warning, false),
            (f32::from_bits(1.0_f32.to_bits() + 1), 0xe34b4b, true),
        ] {
            change(
                &fixture,
                &mut visual,
                MeterInput::Last {
                    left: amplitude,
                    right: amplitude,
                },
            );
            for (fill, cap) in [
                ("output-meter-left-fill", "output-meter-left-above"),
                ("output-meter-right-fill", "output-meter-right-above"),
            ] {
                assert_eq!(visual.rendered_background(fill), Some(rgb(color).into()));
                assert_eq!(visual.debug_bounds(cap).is_some(), above);
                if above {
                    assert_eq!(visual.rendered_background(cap), Some(rgb(0xe34b4b).into()));
                }
            }
        }
        for cap in ["output-meter-left-above", "output-meter-right-above"] {
            let bounds = visual.debug_bounds(cap).expect("latched clipping cap");
            visual.simulate_click(bounds.center(), Modifiers::default());
        }
        change(
            &fixture,
            &mut visual,
            MeterInput::Last {
                left: 0.1,
                right: f32::NAN,
            },
        );
        assert_eq!(
            visual.rendered_background("output-meter-left-fill"),
            Some(rgb(signal).into())
        );
        assert!(visual.debug_bounds("output-meter-left-above").is_none());
        assert_eq!(
            visual
                .debug_bounds("output-meter-right-fill")
                .unwrap()
                .size
                .height,
            px(0.0)
        );
        assert!(visual.debug_bounds("output-meter-right-above").is_none());
    }
}

#[gpui::test]
fn fixed_meter_slot_at_minimum_window_preserves_neighbors_and_actual_fill_geometry(
    cx: &mut TestAppContext,
) {
    let window = cx.add_window(|_, _| Fixture {
        input: MeterInput::Offline,
    });
    let fixture = window.root(&mut *cx).unwrap();
    let mut visual = VisualTestContext::from_window(*window, cx);
    visual.simulate_resize(size(px(960.0), px(600.0)));
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    let slot = visual.debug_bounds("output-meter").unwrap();
    let diagnostic = visual.debug_bounds("meter-diagnostics-fixture").unwrap();
    let message = visual.debug_bounds("meter-message-fixture").unwrap();
    assert_eq!(slot.size, size(px(WIDTH), px(HEIGHT)));
    assert_eq!(
        message.size.width,
        px(154.0),
        "six source status children and five gaps"
    );
    assert_eq!(diagnostic.size.width, px(168.0));
    let mut latched = [false; 2];
    for input in [
        MeterInput::Last {
            left: 0.0,
            right: 0.000_001,
        },
        MeterInput::Last {
            left: 0.001,
            right: 0.1,
        },
        MeterInput::Last {
            left: 1.0,
            right: 2.0,
        },
        MeterInput::Last {
            left: f32::MAX,
            right: f32::MAX,
        },
        MeterInput::Last {
            left: f32::NAN,
            right: f32::INFINITY,
        },
        MeterInput::Stopped,
        MeterInput::Fault,
        MeterInput::Offline,
    ] {
        change(&fixture, &mut visual, input);
        assert_eq!(visual.debug_bounds("output-meter").unwrap(), slot);
        assert_eq!(
            visual.debug_bounds("meter-message-fixture").unwrap(),
            message
        );
        assert_eq!(
            visual.debug_bounds("meter-diagnostics-fixture").unwrap(),
            diagnostic
        );
        let label = visual.debug_bounds("output-meter-readout").unwrap();
        assert!(label.origin.x >= slot.origin.x && label.bottom_right().x <= slot.bottom_right().x);
        assert!(label.origin.y >= slot.origin.y && label.bottom_right().y <= slot.bottom_right().y);
        for (channel, amplitude) in match input {
            MeterInput::Last { left, right } => [("left", left), ("right", right)],
            _ => [("left", f32::NAN), ("right", f32::NAN)],
        } {
            let selector = |suffix: &str| {
                Box::leak(format!("output-meter-{channel}{suffix}").into_boxed_str())
                    as &'static str
            };
            let gauge = visual.debug_bounds(selector("")).unwrap();
            let fill = visual.debug_bounds(selector("-fill")).unwrap();
            assert_eq!(gauge.size, size(px(6.0), px(BAR_HEIGHT)));
            assert!(
                (f32::from(fill.size.height)
                    - (BAR_HEIGHT * peak(amplitude).fraction() * scale).round() / scale)
                    .abs()
                    < 0.01,
                "input={input:?} channel={channel} actual={} expected={}",
                f32::from(fill.size.height),
                BAR_HEIGHT * peak(amplitude).fraction()
            );
            assert_eq!(
                fill.bottom_right().y,
                gauge.bottom_right().y,
                "bottom anchored scale"
            );
            let index = usize::from(channel == "right");
            latched[index] |= peak(amplitude).above();
            assert_eq!(
                visual.debug_bounds(selector("-above")).is_some(),
                latched[index]
            );
        }
    }
    // The cap holds observed clipping through lower levels until explicitly cleared.
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 2.0,
            right: 2.0,
        },
    );
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 0.5,
            right: 0.5,
        },
    );
    let fill = visual.debug_bounds("output-meter-left-fill").unwrap();
    let cap = visual
        .debug_bounds("output-meter-left-above")
        .expect("latched clipping");
    visual.simulate_click(cap.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("output-meter-left-above").is_none());
    assert!(visual.debug_bounds("output-meter-right-above").is_some());
    assert_eq!(visual.debug_bounds("output-meter-left-fill"), Some(fill));
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 2.0,
            right: 2.0,
        },
    );
    let cap = visual
        .debug_bounds("output-meter-left-above")
        .expect("new clipping rearms");
    visual.simulate_click(cap.center(), Modifiers::default());
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 2.0,
            right: 2.0,
        },
    );
    assert!(
        visual.debug_bounds("output-meter-left-above").is_none(),
        "acknowledged stale high reading stays cleared"
    );
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 0.5,
            right: 0.5,
        },
    );
    change(
        &fixture,
        &mut visual,
        MeterInput::Last {
            left: 2.0,
            right: 2.0,
        },
    );
    assert!(
        visual.debug_bounds("output-meter-left-above").is_some(),
        "fresh crossing rearms"
    );
    visual.simulate_resize(size(px(1180.0), px(780.0)));
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("output-meter").unwrap().size, slot.size);
    assert_eq!(
        visual
            .debug_bounds("meter-message-fixture")
            .unwrap()
            .size
            .width,
        px(374.0)
    );
    // TestPlatformNoopText checks element geometry, not Ioskeley glyph fitting.
}
