use super::*;
use crate::{
    ControlStyle, ResponseGraph, ResponseGraphAnalyzerStyle, ResponseGraphModel, ResponseGraphNode,
    SpectrumPlotLayer, SpectrumPlotLayers, layered_spectrum_plot,
};
use gpui::{
    AppContext, Context, Empty, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
    div, prelude::*, size,
};
use std::cell::RefCell;

#[derive(Clone, Debug)]
struct Record {
    plot: Bounds<Pixels>,
    mask: Bounds<Pixels>,
    color: u32,
    pass: Pass,
    alpha: f32,
    commands: Vec<Command>,
    stroke_width: Pixels,
}
thread_local! { static RECORDS: RefCell<Option<Vec<Record>>> = const { RefCell::new(None) }; }
pub(super) fn record(
    plot: Bounds<Pixels>,
    mask: Bounds<Pixels>,
    layer: Layer<'_>,
    pass: Pass,
    alpha: f32,
    commands: Vec<Command>,
) {
    RECORDS.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.push(Record {
                plot,
                mask,
                color: layer.color,
                pass,
                alpha,
                commands,
                stroke_width: layer.stroke_width,
            });
        }
    });
}
fn evaluate(start: [f32; 2], s: Segment, t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    [0, 1].map(|i| {
        u * u * u * start[i]
            + 3.0 * u * u * t * s.a[i]
            + 3.0 * u * t * t * s.b[i]
            + t * t * t * s.end[i]
    })
}
#[test]
fn monotone_geometry_interpolates_calibrated_peaks_without_overshoot_or_backward_x() {
    let hz_x = (1000.0_f32 / 20.0).ln() / (20000.0_f32 / 20.0).ln();
    let level = (-18.0_f32 + 96.0) / 96.0;
    let p = [
        [0.0, 0.0],
        [0.3, 0.12],
        [hz_x, level],
        [0.8, 0.3],
        [1.0, 0.0],
    ];
    assert!(valid_points(&p));
    for i in 0..p.len() - 1 {
        let s = segment(&p, i);
        assert_eq!(evaluate(p[i], s, 0.0), p[i]);
        assert_eq!(evaluate(p[i], s, 1.0), p[i + 1]);
        for j in 0..=100 {
            let q = evaluate(p[i], s, j as f32 / 100.0);
            assert!(q.iter().all(|v| v.is_finite()));
            assert!((p[i][0]..=p[i + 1][0]).contains(&q[0]));
            assert!(
                q[1] >= p[i][1].min(p[i + 1][1]) - 1e-6 && q[1] <= p[i][1].max(p[i + 1][1]) + 1e-6
            );
        }
    }
    assert_eq!(
        segment(&p, 1).end,
        [hz_x, level],
        "exact calibrated 1khz/-18db sample"
    );
    assert_eq!(tangent(&p, 2), 0.0, "no overshoot around actual peak");
}
#[test]
fn degenerate_columns_floor_and_nonfinite_input_stay_bounded() {
    let p = [
        [0.0, 0.0],
        [0.0, 1.0],
        [f32::EPSILON / 2.0, 0.0],
        [1.0, 0.0],
    ];
    assert!(valid_points(&p));
    for i in 0..p.len() - 1 {
        let s = segment(&p, i);
        assert!(
            s.a.into_iter()
                .chain(s.b)
                .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
        );
    }
    assert!(!valid_points(&[[0.0, 0.0], [1.0, f32::NAN]]));
    assert!(!valid_points(&[[1.0, 0.0], [0.0, 1.0]]));
    assert!(!valid_points(&[[0.0, 0.0]; MAX_POINTS + 1]));
    assert!(valid_points(&[[0.0, 0.0], [1.0, 0.0]]));
}
#[test]
fn overlap_budget_keeps_each_layer_nonzero_without_opaque_mud() {
    for n in 1..=MAX_LAYERS {
        let a = fill_alpha(n);
        assert!(a > 0.0 && a <= 0.2);
        assert!(n as f32 * a <= 0.320001);
        assert!(1.0 - (1.0 - a).powi(n as i32) < 0.33);
    }
}
const STYLE: ControlStyle = ControlStyle {
    background: 0x191919,
    surface: 0x222222,
    raised: 0x333333,
    rail: 0x363636,
    border: 0x474747,
    text: 0xeeeeee,
    muted: 0xaaaaaa,
    accent: 0xffffff,
    accent_text: 0x202020,
    numeric_fill: 0x31515a,
    hover_text: 0xe0e0e0,
    focus_marker: 0xaaaaaa,
    dial_rail: 0,
    hover: 0x555555,
    selection: 0x3350a0,
    invalid: 0xff554c,
    font_size: 9.0,
    text_case: crate::TextCase::Preserve,
};
fn peak_points() -> Vec<[f32; 2]> {
    points()
        .into_iter()
        .map(|[x, y]| [x, (y + 0.1).min(1.0)])
        .collect()
}
fn points() -> Vec<[f32; 2]> {
    vec![[0.0, 0.0], [0.3, 0.2], [0.6, 0.8], [1.0, 0.0]]
}
struct Fixture {
    eq: Entity<ResponseGraph>,
    neutral: Entity<ResponseGraph>,
    layers: SpectrumPlotLayers,
    events: Vec<crate::ResponseGraphEvent>,
    _subscription: gpui::Subscription,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .h(px(100.0))
                    .flex_shrink_0()
                    .child(self.eq.clone()),
            )
            .child(
                div()
                    .w_full()
                    .h(px(100.0))
                    .flex_shrink_0()
                    .child(layered_spectrum_plot(self.layers.clone(), STYLE)),
            )
            .child(
                div()
                    .w_full()
                    .h(px(100.0))
                    .flex_shrink_0()
                    .child(self.neutral.clone()),
            )
    }
}
#[gpui::test]
fn production_eq_and_global_submit_the_same_clipped_fill_and_trace_system(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    let handle = *window;
    let fixture = cx
        .update_window(handle, |_, window, cx| {
            let eq = cx.new(|cx| {
                let mut graph = ResponseGraph::new(cx, "spectrum-eq", STYLE);
                graph
                    .set_model(
                        ResponseGraphModel {
                            primary: vec![[0.0, 0.5], [1.0, 0.5]],
                            secondary: vec![[0.0, 0.5], [1.0, 0.5]],
                            analyzer: Some(points()),
                            input_analyzer: Some(points()),
                            analyzer_peak: Some(peak_points()),
                            nodes: vec![ResponseGraphNode {
                                id: 1,
                                position: [0.6, 0.5],
                                enabled: true,
                                selected: true,
                            }],
                        },
                        STYLE,
                        cx,
                    )
                    .unwrap();
                graph
                    .set_analyzer_style(
                        ResponseGraphAnalyzerStyle {
                            input: 0xff6655,
                            output: 0x55ddff,
                        },
                        cx,
                    )
                    .unwrap();
                graph.set_editable(true, cx);
                graph
            });
            let neutral = cx.new(|cx| ResponseGraph::new(cx, "spectrum-neutral-owner", STYLE));
            let layers = SpectrumPlotLayers::new(vec![
                SpectrumPlotLayer {
                    points: points(),
                    peak: None,
                    color: 0xff6655,
                },
                SpectrumPlotLayer {
                    points: points(),
                    peak: Some(peak_points()),
                    color: 0x55ddff,
                },
            ])
            .unwrap();
            window.replace_root(cx, |_, cx| {
                let subscription = cx.subscribe(
                    &eq,
                    |view: &mut Fixture, _, event: &crate::ResponseGraphEvent, _| {
                        view.events.push(*event)
                    },
                );
                Fixture {
                    eq,
                    neutral,
                    layers,
                    events: Vec::new(),
                    _subscription: subscription,
                }
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| {
        assert_eq!(
            window.scale_factor(),
            2.0,
            "actual deterministic 2x paint platform"
        )
    });
    let mut node_position = None;
    for width in [6.0, 8.0, 14.0, 18.0, 120.0, 440.0, 900.0] {
        RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
        visual.simulate_resize(size(px(width), px(300.0)));
        visual.run_until_parked();
        let records = RECORDS.with(|r| r.borrow_mut().take().unwrap());
        if width <= 8.0 {
            assert!(records.is_empty(), "nonpositive plots never submit paths");
            continue;
        }
        if width <= 14.0 {
            assert_eq!(records.len(), 5, "only global plot retains positive width");
            continue;
        }
        assert!(
            !records.is_empty(),
            "actual EQ/global production painters must reach the shared fill system"
        );
        assert_eq!(
            records.len() % 10,
            0,
            "two fills, one output peak and two traces per owner; neutral graph adds none"
        );
        let last = &records[records.len() - 10..];
        write_svg(last, width);
        node_position = Some(map_point(last[0].plot, [0.6, 0.5]));
        for owner in last.chunks(5) {
            assert_eq!(
                owner.iter().map(|r| r.pass).collect::<Vec<_>>(),
                vec![Pass::Fill, Pass::Fill, Pass::Peak, Pass::Trace, Pass::Trace]
            );
            assert_eq!(
                owner.iter().map(|r| r.color).collect::<Vec<_>>(),
                vec![0xff6655, 0x55ddff, 0x55ddff, 0xff6655, 0x55ddff]
            );
            assert_eq!(
                owner.iter().map(|r| r.stroke_width).collect::<Vec<_>>()[3..],
                [SECONDARY_TRACE_WIDTH, PRIMARY_TRACE_WIDTH],
                "output and master traces are the prominent stroke"
            );
            assert!(
                matches!(owner[2].commands.first(), Some(Command::Move(p)) if *p == peak_points()[0]),
                "the peak pass strokes the held-peak curve, not the level curve"
            );
            for r in owner {
                assert_eq!(r.plot, r.mask);
                assert!(r.plot.size.width > px(0.0));
                assert_eq!(r.alpha, fill_alpha(2));
            }
        }
        assert!(
            last[0].plot.top() < last[5].plot.top(),
            "both independent real plot rectangles recorded"
        );
    }
    visual.update(|window, _| window.activate_window());
    visual.simulate_click(node_position.unwrap(), gpui::Modifiers::default());
    visual.run_until_parked();
    fixture.read_with(cx, |view, _| {
        assert!(
            view.events
                .iter()
                .any(|e| e.node_id == 1 && e.phase == crate::ResponseGraphPhase::Press),
            "filled paths do not intercept real node press"
        );
        assert!(
            view.events
                .iter()
                .any(|e| e.node_id == 1 && e.phase == crate::ResponseGraphPhase::Release),
            "node release still reaches owner"
        );
        assert!(
            !view.events.iter().any(|e| matches!(
                e.phase,
                crate::ResponseGraphPhase::Create | crate::ResponseGraphPhase::Begin
            )),
            "click is not a project edit"
        );
    });
    RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
    visual.simulate_resize(size(px(440.0), px(150.0)));
    visual.run_until_parked();
    let clipped = RECORDS.with(|r| r.borrow_mut().take().unwrap());
    assert!(!clipped.is_empty());
    let viewport = Bounds::new(point(px(0.0), px(0.0)), size(px(440.0), px(150.0)));
    assert!(
        clipped
            .iter()
            .any(|r| r.mask.size.height < r.plot.size.height),
        "actual ancestor clip truncates lower plot"
    );
    for r in clipped {
        assert_eq!(
            r.mask,
            r.plot.intersect(&viewport),
            "fill and trace preserve ancestor mask"
        );
    }
    // The real owners clear their measured data after stale/missing output.
    // Empty snapshots must immediately remove every submitted fill and trace.
    RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
    fixture.update(cx, |view, cx| {
        view.layers = SpectrumPlotLayers::default();
        view.eq.update(cx, |graph, cx| {
            graph
                .set_model(
                    ResponseGraphModel {
                        primary: vec![[0.0, 0.5], [1.0, 0.5]],
                        secondary: vec![[0.0, 0.5], [1.0, 0.5]],
                        analyzer: None,
                        input_analyzer: None,
                        analyzer_peak: None,
                        nodes: Vec::new(),
                    },
                    STYLE,
                    cx,
                )
                .unwrap();
        });
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        RECORDS.with(|r| r.borrow_mut().take().unwrap()).is_empty(),
        "no stale spectrum path retained"
    );
}

// A diagnostic of commands actually submitted by both production components.
// It is not a GPUI framebuffer/font/native visual acceptance screenshot.
fn write_svg(records: &[Record], width: f32) {
    use std::fmt::Write;
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='200' viewBox='0 0 {width} 200'><rect width='100%' height='100%' fill='#191919'/>"
    );
    for (i, r) in records.iter().enumerate() {
        let p = r.plot;
        let mut d = String::new();
        let xy = |v| {
            let p = map_point(r.plot, v);
            format!("{} {}", f32::from(p.x), f32::from(p.y))
        };
        for c in &r.commands {
            match c {
                Command::Move(p) => write!(d, "M{} ", xy(*p)),
                Command::Line(p) => write!(d, "L{} ", xy(*p)),
                Command::Cubic(s) => write!(d, "C{} {} {} ", xy(s.a), xy(s.b), xy(s.end)),
                Command::Close => write!(d, "Z"),
            }
            .unwrap();
        }
        write!(svg,"<defs><clipPath id='clip{i}'><rect x='{}' y='{}' width='{}' height='{}'/></clipPath><linearGradient id='fill{i}' x1='0' y1='0' x2='0' y2='1'><stop stop-color='#{:06x}' stop-opacity='{}'/><stop offset='1' stop-color='#{:06x}' stop-opacity='0'/></linearGradient></defs>",f32::from(p.origin.x),f32::from(p.origin.y),f32::from(p.size.width),f32::from(p.size.height),r.color,r.alpha,r.color).unwrap();
        match r.pass {
            Pass::Fill=>write!(svg,"<path clip-path='url(#clip{i})' d='{d}' fill='url(#fill{i})'/>"),
            Pass::Peak=>write!(svg,"<path clip-path='url(#clip{i})' d='{d}' fill='none' stroke='#{:06x}' stroke-opacity='{PEAK_OPACITY}' stroke-width='{}'/>",r.color,f32::from(PEAK_WIDTH)),
            Pass::Trace=>write!(svg,"<path clip-path='url(#clip{i})' d='{d}' fill='none' stroke='#{:06x}' stroke-width='{}'/>",r.color,f32::from(r.stroke_width)),
        }.unwrap();
    }
    svg.push_str("</svg>");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/spectrum-path-review")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("production-spectrum-paths-{width}.svg"));
    std::fs::write(&path, svg).unwrap();
    eprintln!(
        "GEOMETRY DIAGNOSTIC (not native pixels): {}",
        path.display()
    );
}

struct BudgetFixture {
    layers: SpectrumPlotLayers,
}
impl Render for BudgetFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        layered_spectrum_plot(self.layers.clone(), STYLE)
    }
}

#[gpui::test]
fn production_active_fill_budget_ignores_silent_and_invalid_layers(cx: &mut TestAppContext) {
    let handle = *cx.add_window(|_, _| Empty);
    let fixture = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |_, _| BudgetFixture {
                layers: SpectrumPlotLayers::default(),
            })
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(440.0), px(200.0)));
    visual.run_until_parked();
    let active = SpectrumPlotLayer {
        points: points(),
        peak: None,
        color: 0xff6655,
    };
    let floor = SpectrumPlotLayer {
        points: vec![[0.0, 0.0], [1.0, 0.0]],
        peak: None,
        color: 0x55ddff,
    };
    let invalid = SpectrumPlotLayer {
        color: 0x1000000,
        ..active.clone()
    };
    let mut baseline_geometry = None;
    for (case, active_count, floor_count, invalid_count) in [
        ("single", 1, 0, 0),
        ("silent", 1, 256, 0),
        ("mixed", 1, 128, 128),
        ("two-active", 2, 127, 128),
        ("all-inactive", 0, 128, 129),
    ] {
        let layers = [
            vec![active.clone(); active_count],
            vec![floor.clone(); floor_count],
            vec![invalid.clone(); invalid_count],
        ]
        .concat();
        assert!(layers.len() <= 257);
        RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
        fixture.update(cx, |view, cx| {
            view.layers = SpectrumPlotLayers::new(layers).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let records = RECORDS.with(|r| r.borrow_mut().take().unwrap());
        let fills = records
            .iter()
            .filter(|r| r.pass == Pass::Fill)
            .collect::<Vec<_>>();
        if active_count == 0 {
            assert!(
                fills.is_empty(),
                "{case}: inactive layers must not paint a fill"
            );
            continue;
        }
        assert!(
            !fills.is_empty(),
            "{case}: actual production path must paint"
        );
        assert_eq!(fills.len() % active_count, 0);
        for fill in fills {
            let expected = fill_alpha(active_count);
            assert_eq!(
                fill.alpha, expected,
                "{case}: only valid nonfloor spectra spend the fill budget"
            );
            assert_eq!(fill.color, active.color);
            assert_eq!(fill.mask, fill.plot);
            let geometry = format!("{:?}", fill.commands);
            if let Some(baseline) = &baseline_geometry {
                assert_eq!(
                    &geometry, baseline,
                    "{case}: active path geometry stays exact"
                );
            } else {
                baseline_geometry = Some(geometry);
            }
        }
        assert!(records.iter().all(|r| r.color <= 0xffffff));
    }
}

struct RawBudgetFixture {
    layers: Vec<(Vec<[f32; 2]>, u32, Pixels)>,
}
impl Render for RawBudgetFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let layers = self.layers.clone();
        div().size_full().child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    paint(
                        bounds,
                        layers.iter().map(|(points, color, stroke_width)| Layer {
                            points,
                            peak: None,
                            color: *color,
                            stroke_width: *stroke_width,
                        }),
                        Pass::Fill,
                        window,
                    );
                },
            )
            .size_full(),
        )
    }
}

#[gpui::test]
fn shared_fill_budget_skips_malformed_layers_without_bypassing_total_input_cap(
    cx: &mut TestAppContext,
) {
    let handle = *cx.add_window(|_, _| Empty);
    let fixture = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |_, _| RawBudgetFixture { layers: Vec::new() })
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(440.0), px(200.0)));
    visual.run_until_parked();
    let invalid = [
        (vec![[0.0, 0.0], [1.0, f32::NAN]], 0x55ddff, px(1.0)),
        (vec![[1.0, 0.0], [0.0, 0.5]], 0x55ddff, px(1.0)),
        (points(), 0x1000000, px(1.0)),
        (points(), 0x55ddff, px(f32::NAN)),
        (points(), 0x55ddff, px(-1.0)),
        (points(), 0x55ddff, px(5.0)),
        (vec![[0.0, 0.0], [1.0, 0.0]], 0x55ddff, px(1.0)),
    ];
    for (supplied, active_count) in [(257, 1), (258, 1), (257, 0)] {
        let mut layers = (0..supplied)
            .map(|i| invalid[i % invalid.len()].clone())
            .collect::<Vec<_>>();
        if active_count == 1 {
            layers[0] = (points(), 0xff6655, px(1.0));
        }
        RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
        fixture.update(cx, |view, cx| {
            view.layers = layers;
            cx.notify();
        });
        visual.run_until_parked();
        let records = RECORDS.with(|r| r.borrow_mut().take().unwrap());
        if supplied > 257 || active_count == 0 {
            assert!(
                records.is_empty(),
                "total {supplied}/active {active_count}: cap and empty output remain strict"
            );
        } else {
            assert!(
                !records.is_empty(),
                "actual shared painter must submit valid active path"
            );
            for record in records {
                assert_eq!(record.pass, Pass::Fill);
                assert_eq!(record.color, 0xff6655);
                assert_eq!(
                    record.alpha,
                    fill_alpha(1),
                    "malformed/floor layers cannot spend ink"
                );
                assert_eq!(record.mask, record.plot);
            }
        }
    }
}

#[test]
fn empty_and_single_point_layers_never_panic() {
    for pass in [Pass::Fill, Pass::Peak, Pass::Trace] {
        let mut emitted = 0;
        commands(&[], pass, |_| emitted += 1);
        assert_eq!(emitted, 0);
        let mut single = Vec::new();
        commands(&[[1.0, 2.0]], pass, |command| single.push(command));
        assert!(matches!(single.first(), Some(Command::Move([1.0, 2.0]))));
    }
}
