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
    let geometry = Rc::new(WaveformGeometry::mono(channel(&upper, &lower).unwrap()));
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
    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
    }
    assert!(Rc::ptr_eq(
        &geometry,
        &view.read_with(cx, |view, _| view.geometry.clone())
    ));
}
