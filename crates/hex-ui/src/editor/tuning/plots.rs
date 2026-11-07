//! Cached-control-scale plots; no DSP, I/O or authoritative editing.
use super::*;
use gpui::{App, Bounds, PathBuilder, Pixels, Window, fill, point};
use hex_tuning::analysis::pair_fit;
fn quick_scores(scale: Scale) -> (f32, f32) {
    let mut total = 0.0;
    let mut root_total = 0.0;
    for a in 0..scale.count() {
        for b in a + 1..scale.count() {
            let fit = pair_fit((scale.cents()[a] - scale.cents()[b]).abs());
            total += fit;
            if a == scale.root_degree() || b == scale.root_degree() {
                root_total += fit;
            }
        }
    }
    (
        total / (scale.count() * (scale.count() - 1) / 2) as f32,
        root_total / (scale.count() - 1) as f32,
    )
}

pub(super) fn variations(base: Scale, max_move: f32, seed: u64) -> Vec<Scale> {
    let mut choices = vec![base];
    for direction in [-1.0_f32, 1.0] {
        for amount in [0.2, 0.4, 0.7, 1.0].map(|fraction| fraction * max_move) {
            let mut cohesion = base;
            let mut gravity = base;
            for index in 1..base.count() {
                let mut best_cohesion = (f32::NEG_INFINITY, cohesion.cents()[index]);
                let mut best_gravity = (f32::NEG_INFINITY, gravity.cents()[index]);
                for offset in -8..=8 {
                    let move_cents = direction * amount * offset as f32 / 8.0;
                    if move_cents * direction < 0.0 {
                        continue;
                    }
                    let mut trial = base;
                    trial.move_degree(index, base.cents()[index] + move_cents);
                    let (cohesion_score, gravity_score) = quick_scores(trial);
                    let score_c = cohesion_score * direction;
                    let score_g = gravity_score * direction;
                    if score_c > best_cohesion.0 {
                        best_cohesion = (score_c, trial.cents()[index]);
                    }
                    if score_g > best_gravity.0 {
                        best_gravity = (score_g, trial.cents()[index]);
                    }
                }
                cohesion.move_degree(index, best_cohesion.1);
                gravity.move_degree(index, best_gravity.1);
            }
            if !choices.contains(&cohesion) {
                choices.push(cohesion);
            }
            if !choices.contains(&gravity) {
                choices.push(gravity);
            }
        }
    }
    let anchors = choices.clone();
    let mut entropy = seed;
    for anchor in anchors.iter().skip(1) {
        for amount in [0.25_f32, 0.5, 0.75] {
            // SplitMix64 makes a reproducible field for each UI-owned dice seed.
            // It is control-worker work; selection remains a canonical command.
            let amount = if seed == 0 {
                amount
            } else {
                entropy = entropy.wrapping_add(0x9e3779b97f4a7c15);
                let mut word = entropy;
                word = (word ^ (word >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                word = (word ^ (word >> 27)).wrapping_mul(0x94d049bb133111eb);
                word ^= word >> 31;
                0.1 + 0.8 * ((word >> 40) as f32 / 16_777_216.0)
            };
            let mut between = base;
            for index in 1..base.count() {
                between.move_degree(
                    index,
                    base.cents()[index] + (anchor.cents()[index] - base.cents()[index]) * amount,
                );
            }
            if !choices.contains(&between) {
                choices.push(between);
            }
        }
    }
    choices
}

fn color(role: theme::PaletteRole) -> u32 {
    theme::color(role)
}
fn line(window: &mut Window, a: (f32, f32), b: (f32, f32), role: theme::PaletteRole, width: f32) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(point(px(a.0), px(a.1)));
    path.line_to(point(px(b.0), px(b.1)));
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color(role)));
    }
}
fn dot(window: &mut Window, x: f32, y: f32, r: f32, role: theme::PaletteRole) {
    window.paint_quad(
        fill(
            Bounds::from_corners(point(px(x - r), px(y - r)), point(px(x + r), px(y + r))),
            rgb(color(role)),
        )
        .corner_radii(px(r)),
    );
}
fn label(
    window: &mut Window,
    cx: &mut App,
    text: String,
    x: f32,
    y: f32,
    role: theme::PaletteRole,
) {
    let text = theme::text(text);
    let run = gpui::TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color: rgb(color(role)).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(text, px(10.0), &[run], None);
    let _ = shaped.paint(point(px(x), px(y)), px(10.0), window, cx);
}
pub(super) fn pitch(
    bounds: Bounds<Pixels>,
    s: Scale,
    selected: usize,
    activity: [(bool, bool); 48],
    window: &mut Window,
    cx: &mut App,
) {
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let width = f32::from(bounds.size.width);
    let mid = top + 48.0;
    line(
        window,
        (left, mid),
        (left + width, mid),
        theme::PaletteRole::Border,
        1.0,
    );
    for (i, activity) in activity.iter().copied().enumerate().take(s.count()) {
        let x = left + s.cents()[i] / s.period_cents() * width;
        let role = if activity.0 {
            theme::PaletteRole::Waveform
        } else if activity.1 {
            theme::PaletteRole::Secondary
        } else if i == selected {
            theme::PaletteRole::Primary
        } else if i == s.root_degree() {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Text
        };
        line(
            window,
            (x, mid - 12.0),
            (x, mid + 12.0),
            role,
            if i == selected { 2.0 } else { 1.0 },
        );
        dot(window, x, mid, if i == selected { 5.0 } else { 3.0 }, role);
        label(window, cx, format!("{}", i + 1), x - 3.0, mid - 31.0, role);
        if s.count() <= 16 || i == selected {
            label(
                window,
                cx,
                format!("{:.1}¢", s.cents()[i]),
                x - 15.0,
                mid + 17.0,
                theme::PaletteRole::Muted,
            );
        }
    }
    if let Some(hz) = s.degree_frequency(selected) {
        label(
            window,
            cx,
            format!(
                "node {} · {:.3} hz · {:.2} cents",
                selected + 1,
                hz,
                s.cents()[selected]
            ),
            left + 5.0,
            top + 84.0,
            theme::PaletteRole::Primary,
        );
    }
}
pub(super) fn palette(view: &EditorView, cx: &mut Context<EditorView>) -> Stateful<Div> {
    let choices = view.tuning.choices.clone();
    let positions = view.tuning.positions.clone();
    let draw_positions = positions.clone();
    div()
        .id("tuning-pitch-space")
        .h(px(180.0))
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                let _ = window;
                if view.tuning.analysis_pending || view.tuning.drag.is_some() {
                    return;
                }
                // Pick against the exact painted geometry retained by canvas prepaint.
                let Some(bounds) = view.tuning.palette_bounds else {
                    return;
                };
                let x = ((f32::from(event.position.x) - f32::from(bounds.left()))
                    / f32::from(bounds.size.width).max(1.0))
                .clamp(0.0, 1.0);
                let y = ((f32::from(event.position.y) - f32::from(bounds.top()))
                    / f32::from(bounds.size.height).max(1.0))
                .clamp(0.0, 1.0);
                if let Some((i, _)) = positions.iter().enumerate().min_by(|(_, a), (_, b)| {
                    ((a.0 - x).powi(2) + (a.1 - y).powi(2))
                        .total_cmp(&((b.0 - x).powi(2) + (b.1 - y).powi(2)))
                }) && let Some(scale) = choices.get(i)
                    && let Ok(next) = ProjectTuning::new(
                        view.tuning.value.enabled(),
                        *scale,
                        view.tuning.value.audition(),
                    )
                {
                    view.tuning_change(next, NumericEventPhase::Commit);
                    view.refresh_tuning_variations(cx);
                    cx.notify();
                }
            }),
        )
        .child(
            canvas(
                {
                    let owner = cx.entity();
                    move |bounds, _, cx| {
                        owner.update(cx, |view, _| view.tuning.palette_bounds = Some(bounds))
                    }
                },
                move |bounds, _, window, cx| {
                    let left = f32::from(bounds.left());
                    let top = f32::from(bounds.top());
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    label(
                        window,
                        cx,
                        "root affinity ↑ · cohesion →".into(),
                        left + 4.0,
                        top + 4.0,
                        theme::PaletteRole::Muted,
                    );
                    for (i, (x, y)) in draw_positions.iter().copied().enumerate() {
                        dot(
                            window,
                            left + x * w,
                            top + y * h,
                            if i == 0 { 5.0 } else { 3.0 },
                            if i == 0 {
                                theme::PaletteRole::Primary
                            } else {
                                theme::PaletteRole::Secondary
                            },
                        );
                    }
                },
            )
            .size_full(),
        )
}
pub(super) fn positions(choices: &[Scale]) -> Vec<(f32, f32)> {
    let scores: Vec<_> = choices.iter().map(|s| quick_scores(*s)).collect();
    let min_x = scores.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let max_x = scores.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let min_y = scores.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = scores.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    scores
        .into_iter()
        .map(|(x, y)| {
            (
                0.08 + 0.84 * (x - min_x) / (max_x - min_x).max(0.001),
                0.92 - 0.76 * (y - min_y) / (max_y - min_y).max(0.001),
            )
        })
        .collect()
}
pub(super) fn lattice(view: &EditorView, _cx: &mut Context<EditorView>) -> Div {
    let s = view.tuning.value.scale();
    let selected = view.tuning.selected;
    let pair = view.tuning.pair.min(s.count() - 1);
    div().h(px(165.0)).w_full().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let center = (f32::from(bounds.center().x), f32::from(bounds.center().y));
                let radius =
                    (f32::from(bounds.size.height) * 0.37).min(f32::from(bounds.size.width) * 0.4);
                let positions: Vec<_> = (0..s.count())
                    .map(|i| {
                        let angle = s.cents()[i] / s.period_cents() * std::f32::consts::TAU
                            - std::f32::consts::FRAC_PI_2;
                        (
                            center.0 + angle.cos() * radius,
                            center.1 + angle.sin() * radius,
                        )
                    })
                    .collect();
                for i in 0..s.count() {
                    if i != selected {
                        line(
                            window,
                            positions[selected],
                            positions[i],
                            if pair_fit((s.cents()[i] - s.cents()[selected]).abs()) > 0.45 {
                                theme::PaletteRole::Secondary
                            } else {
                                theme::PaletteRole::Border
                            },
                            1.0,
                        );
                    }
                }
                for (i, (x, y)) in positions.into_iter().enumerate() {
                    let role = if i == selected {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::Text
                    };
                    dot(window, x, y, if i == selected { 5.0 } else { 3.0 }, role);
                    label(window, cx, format!("{}", i + 1), x + 6.0, y - 5.0, role);
                }
                let (point, error) = nearest_lattice((s.cents()[selected] - s.cents()[pair]).abs());
                let (n, d) = point.ratio();
                label(
                    window,
                    cx,
                    format!("{}:{} · {:+.1}¢", n, d, error),
                    center.0 - 40.0,
                    center.1 - 5.0,
                    theme::PaletteRole::Muted,
                );
            },
        )
        .size_full(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variation_field_is_bounded_ordered_and_preserves_reference() {
        let s = Scale::default();
        let choices = variations(s, 200.0, 0);
        assert_eq!(choices[0], s);
        assert!(choices.len() <= 65);
        assert!(choices.iter().all(|c| c.valid()
            && c.count() == s.count()
            && c.root_hz() == s.root_hz()
            && c.root_key() == s.root_key()));
        assert!(positions(&choices).iter().all(|(x, y)| x.is_finite()
            && y.is_finite()
            && (0.0..=1.0).contains(x)
            && (0.0..=1.0).contains(y)));
    }
}
