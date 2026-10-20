use super::super::waveform::button;
use super::*;
use crate::theme;
use gpui::{
    Bounds, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, canvas,
    div, point as gpui_point, prelude::*, px, rgb,
};
use hex_project::GainInterpolation;

pub(in crate::editor) fn view(
    view: &mut EditorView,
    viewport: Viewport,
    width: f32,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    let track_id = view.gain_lane.track?;
    let state = view.state.as_ref()?;
    let cursor_frame = view.cursor_position();
    let selected = view.gain_lane.selected;
    let draft = view.gain_lane.draft;
    let generation = view.project_generation;
    let (revision, point, details, title, grid, vertices, points, cursor_gain, unavailable) = state
        .read_with(|snapshot| {
            let project = snapshot.model();
            let track = project.track(track_id)?;
            let point = track
                .gain_points()
                .iter()
                .find(|point| Some(point.beat) == selected)
                .copied();
            let details = point
                .and_then(|point| {
                    let frame = project.tempo().beat_to_frame(point.beat).ok()?;
                    let position = project.meter().position(point.beat).ok()?;
                    Some(format!(
                        "beat {:.6} · frame {} · {}.{} · raw {}",
                        point.beat.raw() as f64 / BeatTime::ONE.raw() as f64,
                        frame,
                        position.bar(),
                        position.beat(),
                        point.beat.raw()
                    ))
                })
                .unwrap_or_else(|| {
                    "click curve to add · drag a point · cmd for free beat placement".into()
                });
            let title = format!(
                "gain · {} · {} · static fallback {:.0}%",
                track.name(),
                if track.gain_points().is_empty() && track.gain_spans().next().is_none() {
                    "static"
                } else {
                    "automation owns gain"
                },
                track.gain() * 100.0
            );
            let grid =
                super::super::musical_time::grid_lines(&project, viewport, width, view.snap_grid);
            let mut vertices = Vec::new();
            let available = viewport.end().and_then(|end| {
                snapshot.visit_gain_preview_segments(track_id, viewport.start, end, |segment| {
                    if let (Some(start), Some(end)) = (
                        viewport.fraction(segment.start_frame),
                        viewport.fraction(segment.end_frame),
                    ) {
                        vertices.push((start, segment.start_gain));
                        vertices.push((end, segment.end_gain));
                    }
                })
            });
            if available == Some(false) {
                vertices.extend([(0.0, track.gain()), (1.0, track.gain())]);
            }
            let vertices = available.map(|_| vertices);
            let points = track
                .gain_points()
                .iter()
                .chain(draft.iter().map(|draft| &draft.point))
                .filter_map(|point| {
                    let frame = project.tempo().beat_to_frame(point.beat).ok()?;
                    Some((
                        point.beat,
                        point.gain,
                        viewport.fraction(frame)?,
                        draft.is_some_and(|draft| draft.point.beat == point.beat),
                    ))
                })
                .collect::<Vec<_>>();
            Some((
                snapshot.revision(),
                point,
                details,
                title,
                grid,
                vertices,
                points,
                snapshot.gain_preview_at(track_id, cursor_frame),
                snapshot
                    .gain_preview_error()
                    .map(|reason| format!("preview unavailable · {reason}")),
            ))
        })
        .ok()
        .flatten()?;
    view.gain_lane.plot_width = width;
    let local_numeric = [&view.gain_lane.gain].into_iter().flatten().any(|binding| {
        binding.entity.read(cx).is_editing() || binding.entity.read(cx).is_interacting()
    });
    let mut controls = div()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .overflow_hidden()
        .child(
            button(
                "‹",
                "previous authored point · includes coincident frames",
                ("gain-point-prev", track_id.value()),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                view.cycle_gain_point(false, cx);
                cx.notify();
            })),
        )
        .child(
            button(
                "›",
                "next authored point · includes coincident frames",
                ("gain-point-next", track_id.value()),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                view.cycle_gain_point(true, cx);
                cx.notify();
            })),
        )
        .child(
            button(
                "+",
                "add or upsert point at arrangement cursor",
                ("gain-point-add", track_id.value()),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if !view.gain_action_admitted(cx) {
                    cx.notify();
                    return;
                }
                let point = view.state.as_ref().and_then(|state| {
                    state
                        .read_with(|snapshot| {
                            let project = snapshot.model();
                            let frame = view.cursor_position();
                            let beat = project.tempo().frame_to_beat(frame).ok()?;
                            let viewport =
                                view.visible_viewport(snapshot, snapshot.timeline_frames());
                            let beat = view.snap_grid.snap_beat(
                                &project,
                                beat,
                                viewport,
                                view.gain_lane.plot_width,
                            )?;
                            let gain = snapshot.gain_preview_at(track_id, frame)?;
                            Some(GainPoint {
                                beat,
                                gain,
                                outgoing: GainInterpolation::Linear,
                            })
                        })
                        .ok()
                        .flatten()
                });
                if let Some(point) = point {
                    if view.begin_gain_draft(None, point) {
                        view.finish_gain_draft();
                    }
                } else {
                    view.gain_lane.error =
                        Some("gain preview unavailable · point was not added".into());
                }
                cx.notify();
            })),
        );
    controls = controls.child(super::super::gain_recording::controls(view, track_id, cx));
    if let Some(point) = point {
        let control = controls::numeric(view, cx);
        controls = controls.child(control);
        for (label, mode) in [
            ("step", GainInterpolation::Step),
            ("linear", GainInterpolation::Linear),
        ] {
            controls = controls.child(
                button(
                    label,
                    "point outgoing interpolation",
                    (
                        "gain-mode",
                        if mode == GainInterpolation::Step {
                            0
                        } else {
                            1
                        },
                    ),
                )
                .w(px(if mode == GainInterpolation::Step {
                    34.0
                } else {
                    46.0
                }))
                .text_color(rgb(theme::color(if point.outgoing == mode {
                    theme::PaletteRole::Secondary
                } else {
                    theme::PaletteRole::Muted
                })))
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.gain_action_admitted(cx)
                        && view.begin_gain_draft(
                            Some(point.beat),
                            GainPoint {
                                outgoing: mode,
                                ..point
                            },
                        )
                    {
                        view.finish_gain_draft();
                    }
                    cx.notify();
                })),
            );
        }
        controls = controls.child(
            button(
                "−",
                "remove selected authored point",
                ("gain-point-remove", track_id.value()),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.gain_edit(GainLaneEdit::Remove(point.beat), cx);
                cx.notify();
            })),
        );
    }
    controls = controls
        .child(
            button(
                "clear",
                "clear lane · restore static gain fallback",
                ("gain-lane-clear", track_id.value()),
            )
            .w(px(38.0))
            .on_click(cx.listener(|view, _, _, cx| {
                view.gain_edit(GainLaneEdit::Clear, cx);
                cx.notify();
            })),
        )
        .child(div().flex_1().min_w_0())
        .child(
            button(
                "×",
                "close gain lane · cancel local draft",
                ("gain-lane-close", track_id.value()),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                view.cancel_gain_draft(cx);
                view.gain_lane.track = None;
                cx.notify();
            })),
        );
    let label = presentation(
        view.gain_lane.error.as_deref(),
        draft.is_some() || local_numeric,
        view.edit_pending(),
        revision,
        view.applied_project_revision,
    );
    let entity = cx.entity();
    let plot = div()
        .id("gain-lane-plot")
        .debug_selector(|| "gain-lane-plot".into())
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                    let rect = [
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    ];
                    if rect[2] <= 0.0 || rect[3] <= 0.0 {
                        return;
                    }
                    for line in &grid {
                        if let Some(x) = viewport.fraction(line.frame) {
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(gpui_point(px(rect[0] + rect[2] * x), px(rect[1])));
                            path.line_to(gpui_point(
                                px(rect[0] + rect[2] * x),
                                px(rect[1] + rect[3]),
                            ));
                            if let Ok(path) = path.build() {
                                window.paint_path(
                                    path,
                                    rgb(theme::color(theme::PaletteRole::Border)),
                                );
                            }
                        }
                    }
                    if let Some(vertices) = &vertices {
                        let mut path = PathBuilder::stroke(px(1.0));
                        for (index, (x, gain)) in vertices.iter().copied().enumerate() {
                            let p = gpui_point(
                                px(rect[0] + x * rect[2]),
                                px(rect[1] + rect[3] * (1.0 - gain / 2.0)),
                            );
                            if index == 0 {
                                path.move_to(p)
                            } else {
                                path.line_to(p)
                            }
                        }
                        if let Ok(path) = path.build() {
                            window
                                .paint_path(path, rgb(theme::color(theme::PaletteRole::Secondary)));
                        }
                    }
                    for &(beat, gain, x, is_draft) in &points {
                        let mut path =
                            PathBuilder::stroke(px(if Some(beat) == selected { 3.0 } else { 2.0 }));
                        let x = rect[0] + x * rect[2];
                        let y = rect[1] + rect[3] * (1.0 - gain / 2.0);
                        path.move_to(gpui_point(px(x - 2.0), px(y)));
                        path.line_to(gpui_point(px(x + 2.0), px(y)));
                        if let Ok(path) = path.build() {
                            window.paint_path(
                                path,
                                rgb(theme::color(if is_draft {
                                    theme::PaletteRole::Primary
                                } else {
                                    theme::PaletteRole::Text
                                })),
                            );
                        }
                    }
                    let down = entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.capture()
                            || event.button != MouseButton::Left
                            || !bounds.contains(&event.position)
                        {
                            return;
                        }
                        let accepted = down.update(cx, |view, cx| {
                            let current = view
                                .state
                                .as_ref()
                                .and_then(|state| {
                                    state
                                        .read_with(|snapshot| snapshot.revision() == revision)
                                        .ok()
                                })
                                .unwrap_or(false);
                            if view.project_generation != generation
                                || !current
                                || view.gain_lane.track != Some(track_id)
                            {
                                return false;
                            }
                            if !view.gain_action_admitted(cx) {
                                return false;
                            }
                            view.start_gain_pointer(
                                viewport,
                                rect,
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.modifiers.platform,
                            )
                        });
                        if accepted {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let moved = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() || event.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        if moved.update(cx, |view, _| {
                            view.move_gain_pointer(
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.modifiers.platform,
                            )
                            .unwrap_or(false)
                        }) {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let up = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        if up.update(cx, |view, _| {
                            if view
                                .gain_lane
                                .draft
                                .is_none_or(|draft| draft.pointer.is_none())
                            {
                                return false;
                            }
                            if view
                                .move_gain_pointer(
                                    [f32::from(event.position.x), f32::from(event.position.y)],
                                    event.modifiers.platform,
                                )
                                .is_err()
                            {
                                return false;
                            }
                            view.finish_gain_draft();
                            true
                        }) {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
    let details_tooltip = details.clone();
    let details = div()
        .id("gain-point-readout")
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::waveform::ClipTooltip {
                name: details_tooltip.clone(),
            })
            .into()
        })
        .w(px(view.track_header_width()))
        .flex_shrink_0()
        .text_xs()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(div().h(px(18.0)).truncate().child(theme::text(details)))
        .child(div().h(px(18.0)).truncate().child(theme::text(format!(
                "cursor frame {} · prepared {}",
                view.cursor_position(),
                cursor_gain
                    .map(|gain| format!("{:.2}%", gain * 100.0))
                    .unwrap_or_else(|| "unavailable".into())
            ))));
    Some(
        div()
            .id("gain-lane")
            .debug_selector(|| "gain-lane".into())
            .h(px(96.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(
                div()
                    .h(px(18.0))
                    .text_xs()
                    .truncate()
                    .child(theme::text(title)),
            )
            .child(controls)
            .child(div().h(px(36.0)).flex().child(plot).child(details))
            .child(
                div()
                    .h(px(18.0))
                    .text_xs()
                    .truncate()
                    .child(theme::text(unavailable.unwrap_or(label))),
            ),
    )
}
