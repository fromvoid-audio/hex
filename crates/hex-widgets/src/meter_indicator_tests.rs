use super::*;
use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, Window, size};

const CONTROLS: ControlStyle = ControlStyle {
    background: 0x202020,
    surface: 0x303030,
    raised: 0x383838,
    rail: 0x282828,
    border: 0x505050,
    text: 0xe0e0e0,
    muted: 0xaaaaaa,
    accent: 0x55ddee,
    accent_text: 0x202020,
    numeric_fill: 0x31515a,
    hover_text: 0xe0e0e0,
    focus_marker: 0xaaaaaa,
    dial_rail: 0x000000,
    hover: 0x404040,
    selection: 0x3350a0,
    invalid: 0xff554c,
    font_size: 8.0,
    text_case: crate::TextCase::Preserve,
};
const STYLE: MeterIndicatorStyle = MeterIndicatorStyle {
    controls: CONTROLS,
    signal: 0x55aa88,
    near_peak: 0xffcc33,
    peak: 0xdddddd,
    overload: 0xdd5566,
};
fn labels() -> [SharedString; 9] {
    METER_INDICATOR_TICKS_DBFS.map(|dbfs| format!("{:.0}", -dbfs).into())
}
fn channel(level_dbfs: f32, held_peak_dbfs: Option<f32>, clipping: bool) -> MeterChannelReadout {
    MeterChannelReadout {
        level_dbfs,
        held_peak_dbfs,
        clipping,
    }
}
struct Fixture(MeterIndicatorReadout);
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().h(px(180.0)).child(
                meter_indicator("meter".into(), self.0, labels(), STYLE).expect("valid style"),
            ),
        )
    }
}
#[test]
fn dbfs_scale_and_invalid_style_are_explicit() {
    assert_eq!(fraction(-60.0), Some(0.0));
    assert_eq!(fraction(-30.0), Some(0.5));
    assert_eq!(fraction(0.0), Some(1.0));
    assert_eq!(fraction(-100.0), Some(0.0));
    assert_eq!(fraction(12.0), Some(1.0));
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(fraction(invalid), None);
    }
    let readout = MeterIndicatorReadout::Mono(channel(-12.0, None, false));
    for font_size in [f32::NAN, f32::INFINITY, 0.0, 100.0] {
        let style = MeterIndicatorStyle {
            controls: ControlStyle {
                font_size,
                ..CONTROLS
            },
            ..STYLE
        };
        assert!(meter_indicator("invalid".into(), readout, labels(), style).is_err());
    }
}
#[gpui::test]
fn mono_and_stereo_meter_colors_follow_dbfs_and_supplied_overload(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| {
        Fixture(MeterIndicatorReadout::Mono(channel(
            -30.0,
            Some(-3.0),
            false,
        )))
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    for stereo in [false, true] {
        for (dbfs, clipping, color) in [
            (-30.0, false, STYLE.signal),
            (-6.0, false, STYLE.signal),
            (-0.45, false, STYLE.signal),
            (crate::METER_NEAR_PEAK_DBFS, false, STYLE.near_peak),
            (0.0, false, STYLE.near_peak),
            (6.0, false, STYLE.overload),
            (-12.0, true, STYLE.overload),
        ] {
            view.update(cx, |view, cx| {
                let value = channel(-60.0, None, false);
                view.0 = if stereo {
                    MeterIndicatorReadout::Stereo([value; 2])
                } else {
                    MeterIndicatorReadout::Mono(value)
                };
                cx.notify();
            });
            visual.run_until_parked();
            for index in 0..if stereo { 2 } else { 1 } {
                let above = Box::leak(format!("meter-channel-{index}-above").into_boxed_str());
                if let Some(cap) = visual.debug_bounds(above) {
                    visual.simulate_click(cap.center(), Modifiers::default());
                }
            }
            view.update(cx, |view, cx| {
                let value = channel(dbfs, Some(-3.0), clipping);
                view.0 = if stereo {
                    MeterIndicatorReadout::Stereo([value; 2])
                } else {
                    MeterIndicatorReadout::Mono(value)
                };
                cx.notify();
            });
            visual.run_until_parked();
            for index in 0..if stereo { 2 } else { 1 } {
                let fill = Box::leak(format!("meter-channel-{index}-fill").into_boxed_str());
                let peak = Box::leak(format!("meter-channel-{index}-peak").into_boxed_str());
                let above = Box::leak(format!("meter-channel-{index}-above").into_boxed_str());
                assert_eq!(visual.rendered_background(fill), Some(rgb(color).into()));
                assert_eq!(
                    visual.rendered_background(peak),
                    Some(rgb(STYLE.peak).into())
                );
                assert_eq!(visual.debug_bounds(above).is_some(), clipping || dbfs > 0.0);
                if clipping || dbfs > 0.0 {
                    assert_eq!(
                        visual.rendered_background(above),
                        Some(rgb(STYLE.overload).into())
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn mono_stereo_fills_peaks_overload_and_ticks_render_bounded(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| {
        Fixture(MeterIndicatorReadout::Stereo([
            channel(-30.0, Some(-6.0), false),
            channel(-12.0, Some(0.0), true),
        ]))
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(240.0), px(220.0)));
    visual.run_until_parked();
    let first = visual.debug_bounds("meter-channel-0").unwrap();
    let second = visual.debug_bounds("meter-channel-1").unwrap();
    let fill = visual.debug_bounds("meter-channel-0-fill").unwrap();
    assert_eq!(first.size.width, px(8.0));
    assert!(second.left() > first.right());
    assert!((fill.size.height - first.size.height * 0.5).abs() <= px(1.0));
    assert!(visual.debug_bounds("meter-channel-0-above").is_none());
    let overload = visual.debug_bounds("meter-channel-1-above").unwrap();
    assert!(overload.top() >= second.top() && overload.bottom() <= second.bottom());
    let tick_top = visual.debug_bounds("meter-tick-0").unwrap();
    let tick_bottom = visual.debug_bounds("meter-tick-8").unwrap();
    assert!((tick_top.center().y - first.top()).abs() <= px(1.0));
    assert!((tick_bottom.center().y - first.bottom()).abs() <= px(1.0));
    for peak in [-90.0, -60.0, -0.01, 0.0, 12.0] {
        view.update(cx, |view, cx| {
            view.0 = MeterIndicatorReadout::Mono(channel(-30.0, Some(peak), false));
            cx.notify();
        });
        visual.run_until_parked();
        let well = visual.debug_bounds("meter-channel-0").unwrap();
        let marker = visual.debug_bounds("meter-channel-0-peak").unwrap();
        assert!(marker.top() >= well.top() && marker.bottom() <= well.bottom());
        assert_eq!(marker.size.height, px(1.0));
        assert!(visual.debug_bounds("meter-channel-1").is_none());
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        view.update(cx, |view, cx| {
            view.0 = MeterIndicatorReadout::Mono(channel(invalid, Some(invalid), true));
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("meter-channel-0-fill")
                .unwrap()
                .size
                .height,
            px(0.0)
        );
        assert!(visual.debug_bounds("meter-channel-0-peak").is_none());
        // Invalid signal does not erase the owner's explicit overload state.
        assert!(visual.debug_bounds("meter-channel-0-above").is_some());
    }
}
