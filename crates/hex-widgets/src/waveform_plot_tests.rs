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

struct OffsetFixture {
    translated: bool,
    absolute: Rc<WaveformGeometry>,
    local: Rc<WaveformGeometry>,
}
impl Render for OffsetFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (geometry, offset) = if self.translated {
            (Rc::clone(&self.local), gpui::point(px(20.0), px(0.0)))
        } else {
            (Rc::clone(&self.absolute), gpui::point(px(0.0), px(0.0)))
        };
        div()
            .relative()
            .w(px(60.0))
            .h(px(50.0))
            .overflow_hidden()
            .debug_selector(|| "waveform-clip".into())
            .child(
                div()
                    .absolute()
                    .left(px(-20.0))
                    .w(px(100.0))
                    .h(px(50.0))
                    .child(
                        waveform_plot_with_offset(
                            move |_| Some((geometry, offset)),
                            WaveformColors {
                                waveform: 0x123456,
                                inactive: 0x789abc,
                                centerline: 0x555555,
                            },
                            WaveformEmphasis::Range(Some(WaveformRange::new(0.25, 0.75).unwrap())),
                        )
                        .debug_selector(|| "waveform-path".into()),
                    ),
            )
    }
}

fn rectangle_geometry(left: f32, right: f32) -> Rc<WaveformGeometry> {
    let upper = [point(px(left), px(0.0)), point(px(right), px(0.0))];
    let lower = [point(px(left), px(50.0)), point(px(right), px(50.0))];
    Rc::new(WaveformGeometry::mono(
        channel(&upper, &lower).expect("valid rectangle envelope"),
    ))
}

#[gpui::test]
fn translated_local_path_matches_absolute_mesh_inside_the_live_clip(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| OffsetFixture {
        translated: false,
        absolute: rectangle_geometry(-20.0, 80.0),
        local: rectangle_geometry(-40.0, 60.0),
    });
    let view = handle.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.run_until_parked();
    let absolute = visual.rendered_path_triangles("waveform-path");
    assert!(absolute.len() >= 2);
    assert!(absolute.windows(2).any(|pair| pair[0].mask != pair[1].mask));
    let clip = visual.debug_bounds("waveform-clip").unwrap();
    assert_eq!(clip.size, size(px(60.0), px(50.0)));

    view.update(cx, |view, cx| {
        view.translated = true;
        cx.notify();
    });
    visual.run_until_parked();
    let translated = visual.rendered_path_triangles("waveform-path");
    assert_eq!(translated.len(), absolute.len());
    for (expected, actual) in absolute.iter().zip(&translated) {
        assert_eq!(actual.color, expected.color);
        assert_eq!(actual.mask, expected.mask);
        assert_eq!(actual.vertices, expected.vertices);
    }

    let mask = absolute[0].mask;
    let mask_left = f32::from(mask.origin.x);
    let mask_right = mask_left + f32::from(mask.size.width);
    assert!(
        absolute
            .iter()
            .flat_map(|triangle| triangle.vertices)
            .any(|vertex| f32::from(vertex.x) < mask_left)
    );
    assert!(
        absolute
            .iter()
            .flat_map(|triangle| triangle.vertices)
            .any(|vertex| f32::from(vertex.x) > mask_right)
    );
}
