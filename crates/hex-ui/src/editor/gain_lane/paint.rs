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
    state: &ViewState,
    viewport: Viewport,
    width: f32,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    view.gain_lane.plot_width = width;
    let track_id = view.gain_lane.track?;
    let track = state.project.track(track_id)?;
    let point = view.gain_point();
    let selected = view.gain_lane.selected;
    let draft = view.gain_lane.draft;
    let generation = view.project_generation;
    let revision = state.revision;
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
                if let Some(state) = view.state.as_ref().cloned() {
                    let frame = view.cursor_position();
                    if let Ok(beat) = state.project.tempo_map().frame_to_beat(frame) {
                        let viewport = view.visible_viewport(state.timeline_frames);
                        if let Some(beat) = view.snap_grid.snap_beat(
                            &state.project,
                            beat,
                            viewport,
                            view.gain_lane.plot_width,
                        ) {
                            let Some(gain) = preview_gain(&state, track_id, frame) else {
                                view.gain_lane.error =
                                    Some("gain preview unavailable · point was not added".into());
                                cx.notify();
                                return;
                            };
                            let point = GainPoint {
                                beat,
                                gain,
                                outgoing: GainInterpolation::Linear,
                            };
                            if view.begin_gain_draft(None, point) {
                                view.finish_gain_draft();
                            }
                        }
                    }
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
        state.revision,
        view.applied_project_revision,
    );
    let details = point
        .and_then(|point| {
            let frame = state.project.tempo_map().beat_to_frame(point.beat).ok()?;
            let position = state.project.meter_map().position(point.beat).ok()?;
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
        if track.gain_envelope().is_empty() && track.gain_lane().spans().is_empty() {
            "static"
        } else {
            "automation owns gain"
        },
        track.gain() * 100.0
    );
    let preview = state.gain_preview.clone();
    let grid =
        super::super::musical_time::grid_lines(&state.project, viewport, width, view.snap_grid);
    let static_gain = track.gain();
    let points = track.gain_envelope().to_vec();
    let project = state.project.clone();
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
                    if let Ok(preview) = &preview {
                        let mut vertices = Vec::new();
                        if let Some(end) = viewport.end() {
                            let present =
                                preview.visit_segments(track_id, viewport.start, end, |segment| {
                                    if let (Some(start), Some(end)) = (
                                        viewport.fraction(segment.start_frame),
                                        viewport.fraction(segment.end_frame),
                                    ) {
                                        vertices.push((start, segment.start_gain));
                                        vertices.push((end, segment.end_gain));
                                    }
                                });
                            if !present {
                                vertices.extend([(0.0, static_gain), (1.0, static_gain)]);
                            }
                        }
                        let mut path = PathBuilder::stroke(px(1.0));
                        for (index, (x, gain)) in vertices.into_iter().enumerate() {
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
                    for p in points.iter().chain(draft.iter().map(|draft| &draft.point)) {
                        if let Ok(frame) = project.tempo_map().beat_to_frame(p.beat)
                            && let Some(x) = viewport.fraction(frame)
                        {
                            let mut path = PathBuilder::stroke(px(if Some(p.beat) == selected {
                                3.0
                            } else {
                                2.0
                            }));
                            let x = rect[0] + x * rect[2];
                            let y = rect[1] + rect[3] * (1.0 - p.gain / 2.0);
                            path.move_to(gpui_point(px(x - 2.0), px(y)));
                            path.line_to(gpui_point(px(x + 2.0), px(y)));
                            if let Ok(path) = path.build() {
                                window.paint_path(
                                    path,
                                    rgb(theme::color(
                                        if draft.is_some_and(|draft| draft.point.beat == p.beat) {
                                            theme::PaletteRole::Primary
                                        } else {
                                            theme::PaletteRole::Text
                                        },
                                    )),
                                );
                            }
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
                            if view.project_generation != generation
                                || view
                                    .state
                                    .as_ref()
                                    .is_none_or(|state| state.revision != revision)
                                || view.gain_lane.track != Some(track_id)
                            {
                                return false;
                            }
                            if !view.gain_action_admitted(cx) {
                                return false;
                            }
                            let Some(state) = view.state.clone() else {
                                return false;
                            };
                            view.start_gain_pointer(
                                &state,
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
                            view.move_gain_pointer(
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.modifiers.platform,
                            );
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
        .w(px(super::super::tracks::TRACK_HEADER_WIDTH))
        .flex_shrink_0()
        .text_xs()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(div().h(px(18.0)).truncate().child(theme::text(details)))
        .child(div().h(px(18.0)).truncate().child(theme::text(format!(
                "cursor frame {} · prepared {}",
                view.cursor_position(),
                preview_gain(state, track_id, view.cursor_position())
                    .map(|gain| format!("{:.2}%", gain * 100.0))
                    .unwrap_or_else(|| "unavailable".into())
            ))));
    let unavailable = state
        .gain_preview
        .as_ref()
        .err()
        .map(|reason| format!("preview unavailable · {}", reason.0));
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
