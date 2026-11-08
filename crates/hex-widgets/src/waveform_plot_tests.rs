use super::*;
use gpui::{Context, Render, TestAppContext, VisualTestContext, point, size};
use std::{cell::RefCell, rc::Rc};

fn channel(upper: &[Point<Pixels>], lower: &[Point<Pixels>]) -> Option<WaveformChannel> {
    WaveformChannel::new(
        WaveformEnvelope {
            upper,
            lower,
            mode: WaveformEnvelopeMode::Filled,
            trace_width: px(0.5),
            centerline: None,
        },
        std::iter::empty(),
    )
}

#[test]
fn malformed_envelopes_and_unbounded_flat_spans_are_refused() {
    let upper = [point(px(0.0), px(1.0)), point(px(10.0), px(2.0))];
    let lower = upper.map(|point| point + gpui::point(px(0.0), px(4.0)));
    assert!(channel(&upper, &lower).is_some());
    assert!(channel(&upper, &lower[..1]).is_none());
    assert!(channel(&lower, &upper).is_none());
    assert!(channel(&[upper[1], upper[0]], &[lower[1], lower[0]]).is_none());
    assert!(channel(&[upper[0], point(px(11.0), px(2.0))], &lower).is_none());
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(channel(&[point(px(invalid), px(1.0)), upper[1]], &lower).is_none());
        assert!(channel(&[point(px(0.0), px(invalid)), upper[1]], &lower).is_none());
    }
    assert!(
        channel(
            &[point(px(-f32::MAX), px(0.0)), point(px(f32::MAX), px(0.0))],
            &[point(px(-f32::MAX), px(1.0)), point(px(f32::MAX), px(1.0))]
        )
        .is_none()
    );
    let excessive = vec![upper[0]; MAX_WAVEFORM_ENVELOPE_POINTS + 1];
    assert!(channel(&excessive, &excessive).is_none());
    let envelope = || WaveformEnvelope {
        upper: &upper,
        lower: &lower,
        mode: WaveformEnvelopeMode::Filled,
        trace_width: px(1.0),
        centerline: None,
    };
    assert!(WaveformChannel::new(envelope(), std::iter::repeat([upper[0], upper[1]])).is_none()); // mismatched y is invalid before the iterator can be unbounded.
    let span = [point(px(0.0), px(4.0)), point(px(10.0), px(4.0))];
    assert!(WaveformChannel::new(envelope(), std::iter::repeat(span)).is_none());
    for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut input = envelope();
        input.trace_width = px(width);
        assert!(WaveformChannel::new(input, std::iter::empty()).is_none());
    }
}

#[test]
fn flat_trace_and_mixed_dc_spans_survive_in_mono_and_stereo_displays() {
    let flat = [point(px(0.0), px(7.0)), point(px(20.0), px(7.0))];
    let trace = || {
        WaveformChannel::new(
            WaveformEnvelope {
                upper: &flat,
                lower: &flat,
                mode: WaveformEnvelopeMode::Trace,
                trace_width: px(0.5),
                centerline: None,
            },
            std::iter::empty(),
        )
        .unwrap()
    };
    assert_eq!(WaveformGeometry::mono(trace()).channels().len(), 1);
    assert_eq!(
        WaveformGeometry::stereo([trace(), trace()])
            .channels()
            .len(),
        2
    );
    let upper = [point(px(0.0), px(2.0)), flat[1]];
    let lower = [point(px(0.0), px(10.0)), flat[1]];
    let mixed = WaveformChannel::new(
        WaveformEnvelope {
            upper: &upper,
            lower: &lower,
            mode: WaveformEnvelopeMode::Filled,
            trace_width: px(0.5),
            centerline: None,
        },
        [flat],
    )
    .unwrap();
    assert!(mixed.has_zero_range_trace());
    assert!(!trace().has_zero_range_trace());
}

#[test]
fn emphasis_refuses_invalid_ranges_and_partitions_the_whole_overview() {
    for range in [
        (-0.1, 0.5),
        (0.5, 1.1),
        (0.7, 0.3),
        (f64::NAN, 1.0),
        (0.0, f64::INFINITY),
    ] {
        assert!(WaveformRange::new(range.0, range.1).is_none());
    }
    let colors = WaveformColors {
        waveform: 1,
        inactive: 2,
        centerline: 3,
    };
    assert_eq!(
        regions(
            WaveformEmphasis::Range(WaveformRange::new(0.2, 0.8)),
            colors
        ),
        [(0.0, 0.2, 2), (0.2, 0.8, 1), (0.8, 1.0, 2)]
    );
    assert_eq!(
        regions(WaveformEmphasis::Range(None), colors),
        [(0.0, 0.0, 2), (0.0, 0.0, 1), (0.0, 1.0, 2)]
    );
}

struct Fixture {
    observed: Rc<RefCell<Vec<WaveformViewport>>>,
    geometry: Rc<WaveformGeometry>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let observed = self.observed.clone();
        let geometry = self.geometry.clone();
        div()
            .w(px(180.0))
            .h(px(90.0))
            .relative()
            .overflow_hidden()
            .child(
                waveform_plot(
                    move |viewport| {
                        observed.borrow_mut().push(viewport);
                        Some(geometry)
                    },
                    WaveformColors {
                        waveform: 0x123456,
                        inactive: 0x789abc,
                        centerline: 0x555555,
                    },
                    WaveformEmphasis::Uniform,
                )
                .debug_selector(|| "shared-waveform".into()),
            )
    }
}
#[gpui::test]
fn retained_geometry_fills_its_parent_and_receives_actual_paint_bounds(cx: &mut TestAppContext) {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let upper = [point(px(0.0), px(10.0)), point(px(180.0), px(15.0))];
    let lower = upper.map(|point| point + gpui::point(px(0.0), px(20.0)));
    let lane = WaveformChannel::new(
        WaveformEnvelope {
            upper: &upper,
            lower: &lower,
            mode: WaveformEnvelopeMode::Filled,
            trace_width: px(0.5),
            centerline: Some(WaveformZeroLine {
                points: [point(px(0.0), px(20.3)), point(px(180.0), px(20.3))],
            }),
        },
        std::iter::empty(),
    )
    .unwrap();
    let geometry = Rc::new(WaveformGeometry::mono(lane));
    let handle = cx.add_window(|_, _| Fixture {
        observed: observed.clone(),
        geometry: geometry.clone(),
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("shared-waveform").unwrap();
    assert_eq!(bounds.size, size(px(180.0), px(90.0)));
    assert!(
        observed
            .borrow()
            .iter()
            .all(|paint| paint.bounds == bounds && paint.scale > 0.0)
    );
    assert!(!observed.borrow().is_empty());
    let scale = observed.borrow().last().unwrap().scale;
    let guides: Vec<_> = zero_line_quads(&geometry, scale).collect();
    assert_eq!(guides.len(), 1);
    assert_eq!(f32::from(guides[0].size.height) * scale, 1.0);

    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
    }
    assert!(Rc::ptr_eq(
        &geometry,
        &view.read_with(cx, |view, _| view.geometry.clone())
    ));
}

#[test]
fn zero_line_quad_plan_is_one_device_pixel_per_lane_in_both_envelope_modes() {
    for mode in [WaveformEnvelopeMode::Filled, WaveformEnvelopeMode::Trace] {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let lane = |y| {
                let upper = [point(px(0.2), px(y - 4.0)), point(px(100.8), px(y - 2.0))];
                let lower = upper.map(|p| p + point(px(0.0), px(6.0)));
                WaveformChannel::new(
                    WaveformEnvelope {
                        upper: &upper,
                        lower: &lower,
                        mode,
                        trace_width: px(0.5),
                        centerline: Some(WaveformZeroLine {
                            points: [point(px(0.2), px(y)), point(px(100.8), px(y))],
                        }),
                    },
                    std::iter::empty(),
                )
                .unwrap()
            };
            let geometry = WaveformGeometry::stereo([lane(10.3), lane(30.7)]);
            let lines: Vec<_> = zero_line_quads(&geometry, scale).collect();
            assert_eq!(lines.len(), 2);
            for (line, y) in lines.iter().zip([10.3, 30.7]) {
                assert!((f32::from(line.size.height) * scale - 1.0).abs() < 1e-5);
                for value in [line.left(), line.right(), line.top(), line.bottom()] {
                    let device = f32::from(value) * scale;
                    assert!((device - device.round()).abs() < 1e-4);
                }
                assert!(line.left() >= px(0.2) && line.right() <= px(100.8));
                assert!((f32::from(line.top()) - y).abs() < 1.0 / scale);
            }
        }
    }
}

#[test]
fn zero_line_refuses_malformed_spans_and_invalid_or_overflowing_scales() {
    let flat = [point(px(-0.2), px(7.3)), point(px(20.8), px(7.3))];
    let build = |points| {
        WaveformChannel::new(
            WaveformEnvelope {
                upper: &flat,
                lower: &flat,
                mode: WaveformEnvelopeMode::Trace,
                trace_width: px(1.0),
                centerline: Some(WaveformZeroLine { points }),
            },
            std::iter::empty(),
        )
    };
    assert!(build([flat[1], flat[0]]).is_none());
    assert!(build([flat[0], point(px(20.0), px(8.0))]).is_none());
    assert!(build([flat[0], point(px(f32::NAN), px(7.3))]).is_none());
    let huge = build([point(px(0.0), px(3.3e38)), point(px(2.0e38), px(3.3e38))]).unwrap();
    assert!(huge.zero_line_bounds(1.0e-38).is_none());
    let channel = build(flat).unwrap();
    for scale in [
        0.0,
        -1.0,
        f32::NAN,
        f32::INFINITY,
        f32::MAX,
        f32::MIN_POSITIVE / 16.0,
    ] {
        assert!(channel.zero_line_bounds(scale).is_none());
    }
}

#[test]
fn layer_batches_only_proven_disjoint_geometry_with_conservative_trace_bounds() {
    let lane = |top, bottom, flat: bool| {
        let upper = [point(px(0.0), px(top)), point(px(100.0), px(top))];
        let lower = upper.map(|p| point(p.x, px(bottom)));
        WaveformChannel::new(
            WaveformEnvelope {
                upper: &upper,
                lower: &lower,
                mode: WaveformEnvelopeMode::Filled,
                trace_width: px(2.0),
                centerline: None,
            },
            flat.then_some([point(px(0.0), px(top)), point(px(100.0), px(top))]),
        )
        .unwrap()
    };
    let separate = WaveformGeometry::stereo([lane(10.0, 20.0, false), lane(30.0, 40.0, false)]);
    let layer = envelope_layer(&separate, 2.0).unwrap();
    assert_eq!(layer.origin, point(px(-1.5), px(8.5)));
    assert_eq!(layer.bottom_right(), point(px(101.5), px(41.5)));
    // A lower display scale expands the physical antialiasing margin.
    assert_eq!(envelope_layer(&separate, 0.5).unwrap().top(), px(7.0));
    let overlap = WaveformGeometry::stereo([lane(10.0, 20.0, false), lane(21.0, 30.0, false)]);
    assert!(envelope_layer(&overlap, 2.0).is_none());
    let flat = WaveformGeometry::stereo([lane(10.0, 20.0, true), lane(30.0, 40.0, false)]);
    assert!(envelope_layer(&flat, 2.0).is_none());
    assert!(envelope_layer(&WaveformGeometry::mono(lane(10.0, 20.0, false)), 2.0).is_none());
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(envelope_layer(&separate, scale).is_none());
    }
    let a = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(1.0)));
    let b = Bounds::new(point(px(0.0), px(2.0)), size(px(10.0), px(1.0)));
    assert!(disjoint_layer(a, b).is_some());
    assert!(disjoint_layer(a, a).is_none());
    assert!(disjoint_layer(a, Bounds::new(point(px(0.0), px(f32::INFINITY)), b.size)).is_none());
}
