//! Read-only Compressor displays. The audible endpoint supplies peak values;
//! the editor owns the selected view and a bounded, wall-clock activity window.

use crate::{editor::EditorView, theme};
use gpui::{
    Context, Div, FocusHandle, KeyDownEvent, PathBuilder, canvas, div, point, prelude::*, px,
    relative, rgb,
};
use hex_engine::CompressorTransferCurve;
use hex_project::DeviceId;
use hex_widgets::{LevelMeterStyle, MeterLevel};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const HISTORY_DURATION: Duration = Duration::from_secs(3);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(33);
const MAX_SAMPLE_GAP: Duration = Duration::from_millis(250);
const REDUCTION_RANGE_DB: f32 = 36.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::editor::device_panel) enum Mode {
    Meters,
    #[default]
    Curve,
    Activity,
}

impl Mode {
    const ALL: [Self; 3] = [Self::Meters, Self::Curve, Self::Activity];

    fn label(self) -> &'static str {
        match self {
            Self::Meters => "meters",
            Self::Curve => "curve",
            Self::Activity => "activity",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::editor::device_panel) struct Reading {
    /// Identity of the borrowed audible endpoint. No reference is retained.
    pub source: usize,
    pub input: f32,
    pub output: f32,
    pub reduction: f32,
    pub detector: f32,
}

#[derive(Default)]
struct History {
    points: VecDeque<(Instant, Reading)>,
    last_update: Option<Instant>,
}

impl History {
    fn update(&mut self, now: Instant, reading: Option<Reading>) {
        let Some(reading) = reading else {
            self.points.clear();
            self.last_update = None;
            return;
        };
        if self
            .last_update
            .is_some_and(|last| now - last > MAX_SAMPLE_GAP)
            || self
                .points
                .back()
                .is_some_and(|(_, last)| last.source != reading.source)
        {
            self.points.clear();
        }
        self.last_update = Some(now);
        while self
            .points
            .front()
            .is_some_and(|(at, _)| now - *at > HISTORY_DURATION)
        {
            self.points.pop_front();
        }
        // Merge extra repaints into the current time bin, keeping its peaks.
        // The window and minimum spacing bound the buffer to 91 points.
        if let Some((at, last)) = self.points.back_mut()
            && now - *at < SAMPLE_INTERVAL
        {
            last.input = last.input.max(reading.input);
            last.output = last.output.max(reading.output);
            last.reduction = last.reduction.max(reading.reduction);
        } else {
            self.points.push_back((now, reading));
        }
    }
}

pub(in crate::editor::device_panel) struct DisplayState {
    history: History,
    focus: [FocusHandle; 3],
}

pub(super) fn mode_buttons(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    mode: Mode,
) -> Div {
    let mut row = div().flex().gap(px(3.0)).h(px(15.0)).flex_shrink_0();
    let Some(display) = view.device_panel.compressor_displays.get(&device) else {
        return row;
    };
    for (index, (target, focus)) in Mode::ALL.into_iter().zip(&display.focus).enumerate() {
        let mut style = theme::action_style();
        let focus_color = theme::color(theme::PaletteRole::Primary);
        if mode == target {
            let accent = theme::color(theme::PaletteRole::Secondary);
            style = style.with_state_colors(accent, theme::readable_text(accent, style.text));
        }
        let selector = format!("compressor-view-{}-{}", target.label(), device.value());
        let click_focus = focus.clone();
        row = row.child(
            hex_widgets::device_segment(theme::text(target.label()), style)
                .id(selector.clone())
                .debug_selector(move || selector.clone())
                .w_auto()
                .h_full()
                .px(px(6.0))
                .border_1()
                .border_color(rgb(style.background))
                .focus(move |button| button.border_color(rgb(focus_color)))
                .track_focus(focus)
                .on_click(cx.listener(move |view, _, window, cx| {
                    click_focus.focus(window);
                    view.device_panel.compressor_modes.insert(device, target);
                    cx.notify();
                }))
                .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                    let next = match event.keystroke.key.as_str() {
                        "left" => (index + Mode::ALL.len() - 1) % Mode::ALL.len(),
                        "right" => (index + 1) % Mode::ALL.len(),
                        "enter" | "space" => index,
                        _ => return,
                    };
                    if let Some(display) = view.device_panel.compressor_displays.get(&device) {
                        display.focus[next].focus(window);
                        view.device_panel
                            .compressor_modes
                            .insert(device, Mode::ALL[next]);
                        cx.notify();
                        cx.stop_propagation();
                    }
                })),
        );
    }
    row
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    mode: Mode,
    reading: Option<Reading>,
    threshold_db: Option<f32>,
    curve: Option<Result<CompressorTransferCurve, String>>,
) -> Div {
    let display = view
        .device_panel
        .compressor_displays
        .entry(device)
        .or_insert_with(|| DisplayState {
            history: History::default(),
            focus: std::array::from_fn(|_| cx.focus_handle().tab_index(0).tab_stop(true)),
        });
    let now = Instant::now();
    display.history.update(now, reading);
    let reading = reading.unwrap_or_default();
    match mode {
        Mode::Meters => {
            let selector = format!("compressor-meters-{}", device.value());
            div()
                .debug_selector(move || selector.clone())
                .flex()
                .justify_center()
                .size_full()
                .gap(px(14.0))
                .children(meters(device, reading, threshold_db))
        }
        Mode::Curve => {
            let [input, reduction, output] = meters(device, reading, threshold_db);
            div()
                .flex()
                .size_full()
                .gap(px(4.0))
                .child(input)
                .child(transfer(device, curve, reading.detector))
                .child(reduction)
                .child(output)
        }
        Mode::Activity => activity(device, &display.history, now),
    }
}

fn meters(device: DeviceId, reading: Reading, threshold_db: Option<f32>) -> [Div; 3] {
    let colors = theme::meter_colors();
    let style = theme::device_style(theme::DeviceStyleRole::Display);
    let level_style = LevelMeterStyle {
        background: style.rail,
        fill: colors.signal,
        above: colors.clipping,
    };
    let level = |label, peak, threshold: Option<f32>| {
        let selector = format!("compressor-{label}-meter-{}", device.value());
        let meter = hex_widgets::signal_meter(
            selector.into(),
            MeterLevel::fraction_from_amplitude(peak),
            LevelMeterStyle {
                fill: colors.color(MeterLevel::from_amplitude(peak, false)),
                ..level_style
            },
        );
        let amplitude_db = 20.0 * peak.log10();
        let caption = if amplitude_db.is_finite() {
            format!("{amplitude_db:.1}")
        } else {
            "−∞".to_owned()
        };
        meter_column(
            label,
            caption,
            meter.when_some(threshold, |meter, db| {
                meter.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(relative(((db + 60.0) / 60.0).clamp(0.0, 1.0)))
                        .h(px(1.0))
                        .bg(rgb(colors.near_peak)),
                )
            }),
        )
    };
    [
        level("in", reading.input, threshold_db),
        meter_column(
            "gr",
            format!("{:.1}", -reading.reduction),
            hex_widgets::reduction_meter(
                format!("compressor-gr-meter-{}", device.value()).into(),
                reading.reduction / REDUCTION_RANGE_DB,
                LevelMeterStyle {
                    fill: colors.near_peak,
                    ..level_style
                },
            ),
        ),
        level("out", reading.output, None),
    ]
}

fn meter_column(label: &str, reading: String, meter: Div) -> Div {
    div()
        .w(px(32.0))
        .flex_shrink_0()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.0))
        .child(theme::text(label))
        .child(div().w(px(10.0)).flex_1().min_h_0().child(meter))
        .child(theme::text(reading))
}

fn transfer(
    device: DeviceId,
    curve: Option<Result<CompressorTransferCurve, String>>,
    detector: f32,
) -> Div {
    let graph = div()
        .debug_selector(move || format!("compressor-transfer-{}", device.value()))
        .h_full()
        .flex_1()
        .min_w_0();
    let Some(curve) = curve else {
        return graph;
    };
    let curve = match curve {
        Ok(curve) => curve,
        Err(error) => return graph.child(theme::text(format!("preview unavailable · {error}"))),
    };
    let style = theme::device_style(theme::DeviceStyleRole::Display);
    let Some(plot) =
        hex_widgets::curve_plot(curve.points, style, hex_widgets::CurvePlotGrid::Horizontal)
    else {
        return graph;
    };
    let plot =
        plot.overflow_hidden()
            .when_some(detector_marker(&curve, detector), |plot, [x, y]| {
                plot.child(
                    div()
                        .debug_selector(move || {
                            format!("compressor-detector-marker-{}", device.value())
                        })
                        .absolute()
                        .left(relative(x))
                        .top(relative(1.0 - y))
                        .ml(px(-2.5))
                        .mt(px(-2.5))
                        .size(px(5.0))
                        .rounded_full()
                        .bg(rgb(theme::meter_colors().near_peak)),
                )
            });
    graph.child(plot)
}

fn detector_marker(curve: &CompressorTransferCurve, detector: f32) -> Option<[f32; 2]> {
    if !detector.is_finite() || detector < 0.001 {
        return None;
    }
    let x = MeterLevel::fraction_from_amplitude(detector);
    let index = x * (curve.points.len() - 1) as f32;
    let lower = index.floor() as usize;
    let upper = (lower + 1).min(curve.points.len() - 1);
    let y = curve.points[lower][1];
    Some([x, y + (curve.points[upper][1] - y) * index.fract()])
}

fn activity(device: DeviceId, history: &History, now: Instant) -> Div {
    let colors = theme::meter_colors();
    let style = theme::device_style(theme::DeviceStyleRole::Display);
    let traces = [
        ("in", style.muted),
        ("out", colors.signal),
        ("gr", colors.near_peak),
    ];
    let points: Vec<_> = history
        .points
        .iter()
        .map(|(at, reading)| {
            let x = 1.0 - (now - *at).as_secs_f32() / HISTORY_DURATION.as_secs_f32();
            let y = [
                1.0 - MeterLevel::fraction_from_amplitude(reading.input),
                1.0 - MeterLevel::fraction_from_amplitude(reading.output),
                (reading.reduction / REDUCTION_RANGE_DB).clamp(0.0, 1.0),
            ];
            (x, y)
        })
        .collect();
    div()
        .debug_selector(move || format!("compressor-activity-{}", device.value()))
        .flex()
        .flex_col()
        .size_full()
        .child(
            div()
                .flex()
                .justify_between()
                .flex_shrink_0()
                .px(px(3.0))
                .child(div().flex().gap(px(8.0)).children(
                    traces.map(|(label, color)| {
                        div().text_color(rgb(color)).child(theme::text(label))
                    }),
                ))
                .child(theme::text("3 s")),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .bg(rgb(style.background))
                .border_1()
                .border_color(rgb(style.border))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let position = |x, y| {
                                point(
                                    bounds.origin.x + bounds.size.width * x,
                                    bounds.origin.y + bounds.size.height * y,
                                )
                            };
                            let mut grid = PathBuilder::stroke(px(0.5));
                            for fraction in [0.25, 0.5, 0.75] {
                                grid.move_to(position(0.0, fraction));
                                grid.line_to(position(1.0, fraction));
                            }
                            if let Ok(path) = grid.build() {
                                window.paint_path(path, rgb(style.rail));
                            }
                            for (lane, (_, color)) in traces.into_iter().enumerate() {
                                let mut trace = PathBuilder::stroke(px(1.0));
                                for (index, (x, y)) in points.iter().enumerate() {
                                    if index == 0 {
                                        trace.move_to(position(*x, y[lane]));
                                    } else {
                                        trace.line_to(position(*x, y[lane]));
                                    }
                                }
                                if let Ok(path) = trace.build() {
                                    window.paint_path(path, rgb(color));
                                }
                            }
                        },
                    )
                    .size_full(),
                ),
        )
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

    #[test]
    fn history_combines_peaks_and_retires_unavailable_or_replaced_audio() {
        let mut history = History::default();
        let start = Instant::now();
        let sample = Reading {
            input: 0.5,
            output: 0.25,
            reduction: 6.0,
            ..Reading::default()
        };
        history.update(start, Some(sample));
        let transient = Reading {
            input: 0.25,
            output: 0.4,
            reduction: 7.0,
            ..sample
        };
        history.update(start + Duration::from_millis(10), Some(transient));
        let (_, peak) = history.points[0];
        assert_eq!([peak.input, peak.output, peak.reduction], [0.5, 0.4, 7.0]);
        for index in 1..200 {
            history.update(start + SAMPLE_INTERVAL * index, Some(sample));
        }
        assert_eq!(history.points.len(), 91);
        let replacement = start + SAMPLE_INTERVAL * 200;
        history.update(
            replacement,
            Some(Reading {
                source: 1,
                ..sample
            }),
        );
        assert_eq!(history.points.len(), 1);
        history.update(replacement + SAMPLE_INTERVAL, None);
        assert!(history.points.is_empty());
    }

    #[test]
    fn curve_marker_interpolates_at_the_live_detector_level() {
        let curve = CompressorTransferCurve {
            points: std::array::from_fn(|index| {
                let x = index as f32 / 128.0;
                [x, x * 0.5]
            }),
        };
        let [x, y] = detector_marker(&curve, 10.0_f32.powf(-29.0 / 20.0)).unwrap();
        assert!((x - 31.0 / 60.0).abs() < 1e-6);
        assert!((y - 31.0 / 120.0).abs() < 1e-6);
        assert_eq!(detector_marker(&curve, 0.0), None);
    }
}
