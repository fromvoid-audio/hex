//! Visible-lane composition and pointer projection; canonical edits stay on the
//! worker. Outer track list geometry includes lane space, never stretches clips.
use super::super::viewport::Viewport;
use super::*;
use crate::theme;
use gpui::{
    Bounds, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, canvas,
    div, point, prelude::*, px, rgb,
};

fn lane_button(
    label: &'static str,
    tooltip: &'static str,
    id: SharedString,
) -> gpui::Stateful<Div> {
    hex_widgets::action(theme::text(label), theme::action_style())
        .id(id)
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Help,
                description: tooltip.into(),
            })
            .into()
        })
}

pub(in crate::editor) fn overlay(
    view: &mut EditorView,
    state: &ViewState,
    owner: Owner,
    viewport: Viewport,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    if !view.automation_view.enabled {
        return None;
    }
    let entry = view
        .automation_view
        .devices
        .main
        .iter()
        .find(|entry| entry.target.owner == owner)?
        .clone();
    Some(
        plot(view, state, entry, viewport, true, cx)
            .absolute()
            .inset_0(),
    )
}

pub(in crate::editor) fn rows(
    view: &mut EditorView,
    state: &ViewState,
    owner: Owner,
    viewport: Viewport,
    cx: &mut Context<EditorView>,
) -> Div {
    if !view.automation_view.enabled {
        return div();
    }
    let count = view
        .automation_view
        .devices
        .extra
        .iter()
        .filter(|entry| entry.target.owner == owner)
        .count();
    if count == 0 {
        return div();
    }
    let range = view
        .automation_view
        .devices
        .windows
        .iter()
        .find(|(key, _)| *key == owner)
        .map(|(_, range)| range.clone())
        .unwrap_or(0..count.min(8));
    let entries: Vec<_> = view
        .automation_view
        .devices
        .extra
        .iter()
        .filter(|entry| entry.target.owner == owner)
        .enumerate()
        .skip(range.start)
        .take(range.end.saturating_sub(range.start))
        .map(|(index, entry)| (index, entry.clone()))
        .collect();
    let entity = cx.entity();
    let mut body = div()
        .w_full()
        .h(px(count as f32 * LANE_HEIGHT))
        .flex_shrink_0()
        .relative()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, cx| {
                    let mask = window.content_mask().bounds;
                    let top = f32::from(bounds.origin.y);
                    let first = ((f32::from(mask.origin.y) - top) / LANE_HEIGHT)
                        .floor()
                        .max(0.0) as usize;
                    let end = ((f32::from(mask.bottom()) - top) / LANE_HEIGHT)
                        .ceil()
                        .max(0.0) as usize;
                    let visible =
                        first.saturating_sub(1).min(count)..end.saturating_add(1).min(count);
                    entity.update(cx, |view, cx| {
                        let windows = &mut view.automation_view.devices.windows;
                        if let Some((_, old)) = windows.iter_mut().find(|(key, _)| *key == owner) {
                            if *old != visible {
                                *old = visible;
                                cx.notify();
                            }
                        } else {
                            windows.push((owner, visible));
                            cx.notify();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
    for (index, entry) in entries {
        let target = entry.target;
        let points = view.automation_view.devices.points(state, target);
        let value = view
            .automation_view
            .devices
            .selected
            .filter(|(selected, _)| *selected == target)
            .and_then(|(_, beat)| points.iter().find(|point| point.beat == beat))
            .map_or(entry.base, |point| point.value.get());
        let name = format!("{} · {}", entry.label, entry.format(value));
        let header = div()
            .w(px(view.track_header_width()))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .px(px(3.0))
            .text_xs()
            .border_l_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .child(div().truncate().h(px(22.0)).child(theme::text(name)))
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .child(
                        lane_button(
                            "…",
                            "automation lane options",
                            gpui::SharedString::from(format!(
                                "automation-options-{}-{}",
                                target.device.value(),
                                target.key()
                            )),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                view.open_lane_options(target, event.position, false, window, cx);
                            }),
                        ),
                    )
                    .child(
                        lane_button(
                            "×",
                            "hide lane · keep automation",
                            gpui::SharedString::from(format!(
                                "automation-hide-{}-{}",
                                target.device.value(),
                                target.key()
                            )),
                        )
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.automation_menu_action(Action::Hide(target), window, cx)
                            },
                        )),
                    ),
            );
        body = body.child(
            div()
                .absolute()
                .top(px(index as f32 * LANE_HEIGHT))
                .w_full()
                .h(px(LANE_HEIGHT))
                .flex()
                .debug_selector(move || {
                    format!("automation-lane-{}-{}", target.device.value(), target.key())
                })
                .border_t_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .child(plot(view, state, entry, viewport, false, cx))
                .child(header),
        );
    }
    body
}

fn plot(
    view: &mut EditorView,
    state: &ViewState,
    entry: Entry,
    viewport: Viewport,
    main: bool,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let target = entry.target;
    let all_points = view.automation_view.devices.points(state, target);
    // Include one neighbor at each edge so clipping preserves interpolation.
    let first = all_points
        .partition_point(|point| {
            state
                .project
                .tempo_map()
                .beat_to_frame(point.beat)
                .is_ok_and(|frame| frame < viewport.start)
        })
        .saturating_sub(1);
    let end = all_points
        .partition_point(|point| {
            state
                .project
                .tempo_map()
                .beat_to_frame(point.beat)
                .is_ok_and(|frame| frame <= viewport.end().unwrap_or(u64::MAX))
        })
        .saturating_add(1)
        .min(all_points.len());
    let points = all_points.get(first..end).unwrap_or_default().to_vec();
    let selected = view.automation_view.devices.selected;
    let project = state.project.clone();
    let revision = state.revision;
    let base = entry.base;
    let entity = cx.entity();
    let label = entry.label.clone();
    div()
        .id(gpui::SharedString::from(format!(
            "automation-plot-{}-{}-{main}",
            target.device.value(),
            target.key()
        )))
        .debug_selector(move || {
            format!(
                "automation-plot-{}-{}-{main}",
                target.device.value(),
                target.key()
            )
        })
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .cursor(gpui::CursorStyle::Crosshair)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, cx| {
                    let rect = [
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    ];
                    if rect[2] <= 0.0 || rect[3] <= 0.0 {
                        return;
                    }
                    let xy = |beat: BeatTime, value: f32| -> Option<Point<Pixels>> {
                        let frame = project.tempo_map().beat_to_frame(beat).ok()?;
                        Some(point(
                            px(rect[0]
                                + ((frame as f64 - viewport.start as f64)
                                    / viewport.length.max(1) as f64)
                                    as f32
                                    * rect[2]),
                            px(rect[1] + (1.0 - value) * (rect[3] - 8.0) + 4.0),
                        ))
                    };
                    let color = rgb(theme::color(theme::PaletteRole::Secondary));
                    if points.is_empty() {
                        let y = rect[1] + (1.0 - base) * (rect[3] - 8.0) + 4.0;
                        let mut x = 0.0;
                        while x < rect[2] {
                            let mut path = PathBuilder::stroke(px(1.0));
                            path.move_to(point(px(rect[0] + x), px(y)));
                            path.line_to(point(px(rect[0] + (x + 5.0).min(rect[2])), px(y)));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                            x += 9.0;
                        }
                    } else {
                        let mut path = PathBuilder::stroke(px(1.5));
                        let first = &points[0];
                        let first_value = first.value.get();
                        path.move_to(point(
                            px(rect[0]),
                            px(rect[1] + (1.0 - first_value) * (rect[3] - 8.0) + 4.0),
                        ));
                        let mut previous = None;
                        for value in &points {
                            if let Some(position) = xy(value.beat, value.value.get()) {
                                if let Some((old, shape)) = previous {
                                    if shape == AutomationInterpolation::Step || entry.is_discrete()
                                    {
                                        path.line_to(point(position.x, old));
                                    }
                                }
                                path.line_to(position);
                                previous = Some((position.y, value.outgoing));
                                if bounds.contains(&position) {
                                    let mut marker = PathBuilder::stroke(px(
                                        if selected == Some((target, value.beat)) {
                                            3.0
                                        } else {
                                            2.0
                                        },
                                    ));
                                    marker.move_to(point(position.x - px(2.0), position.y));
                                    marker.line_to(point(position.x + px(2.0), position.y));
                                    if let Ok(marker) = marker.build() {
                                        window.paint_path(marker, color);
                                    }
                                }
                            }
                        }
                        if let Some((y, _)) = previous {
                            path.line_to(point(px(rect[0] + rect[2]), y));
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    }
                    {
                        let view = entity.read(cx);
                        if view
                            .automation_view
                            .devices
                            .draft
                            .as_ref()
                            .is_some_and(|draft| {
                                draft.target == target && draft.revision == revision
                            })
                            && view.project_generation == target.generation
                            && view.focus.is_focused(window)
                            && view.automation_view.enabled
                        {
                            window.renew_pointer_capture(
                                entity.entity_id(),
                                gpui::CursorStyle::Crosshair,
                            );
                        }
                    }
                    let owner = entity.clone();
                    let generation = target.generation;
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.capture() || !bounds.contains(&event.position) {
                            return;
                        }
                        if event.button == MouseButton::Right {
                            owner.update(cx, |view, cx| {
                                view.open_lane_options(target, event.position, main, window, cx)
                            });
                            cx.stop_propagation();
                            return;
                        }
                        if event.button != MouseButton::Left {
                            return;
                        }
                        let accepted = owner.update(cx, |view, cx| {
                            if generation != view.project_generation
                                || view
                                    .state
                                    .as_ref()
                                    .is_none_or(|state| state.revision != revision)
                            {
                                return false;
                            }
                            view.begin_device_automation_pointer(
                                target, viewport, rect, event, window, cx,
                            )
                        });
                        if accepted {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
        .when(main, |plot| {
            plot.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(3.0))
                    .text_xs()
                    .child(theme::text(&label)),
            )
        })
}

impl EditorView {
    pub(super) fn open_lane_options(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        main: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut actions = Vec::new();
        if self
            .automation_view
            .devices
            .selected
            .is_some_and(|(selected, _)| selected == target)
            && self
                .automation_view
                .devices
                .entry(target)
                .is_some_and(|entry| !entry.is_discrete())
        {
            actions.push((
                "linear segment",
                Action::Shape(target, AutomationInterpolation::Linear),
            ));
            actions.push((
                "step segment",
                Action::Shape(target, AutomationInterpolation::Step),
            ));
        }
        actions.push(("clear automation", Action::Clear(target)));
        actions.push((
            "hide lane",
            if main {
                Action::HidePreview(target)
            } else {
                Action::Hide(target)
            },
        ));
        self.open_automation_menu(position, actions, window, cx);
    }

    fn begin_device_automation_pointer(
        &mut self,
        target: Target,
        viewport: Viewport,
        rect: [f32; 4],
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.edit_pending()
            || self.automation_view.devices.pending.is_some()
            || self.automation_view.devices.draft.is_some()
            || !self.arrangement_view_change_available(cx)
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if target
            .current(&state.project, self.project_generation)
            .is_none()
        {
            return false;
        }
        let Some(entry) = self.automation_view.devices.entry(target).cloned() else {
            return false;
        };
        let points = self.automation_view.devices.points(state, target);
        let pos = [f32::from(event.position.x), f32::from(event.position.y)];
        let hit = points
            .iter()
            .find(|value| {
                state
                    .project
                    .tempo_map()
                    .beat_to_frame(value.beat)
                    .ok()
                    .and_then(|frame| viewport.fraction(frame))
                    .is_some_and(|x| {
                        (rect[0] + x * rect[2] - pos[0]).abs() <= 6.0
                            && (rect[1] + (1.0 - value.value.get()) * (rect[3] - 8.0) + 4.0
                                - pos[1])
                                .abs()
                                <= 6.0
                    })
            })
            .copied();
        let Some(value) = pointer_value(
            &state.project,
            &entry,
            self.snap_grid
                .with_platform_modifier(event.modifiers.platform),
            viewport,
            rect,
            pos,
        ) else {
            return false;
        };
        let value = hit.unwrap_or(value);
        let revision = state.revision;
        let mut candidate = points;
        match candidate.binary_search_by_key(&value.beat, |point| point.beat) {
            Ok(index) => candidate[index] = value,
            Err(index) => candidate.insert(index, value),
        }
        if candidate.len() > hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
            return false;
        }
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            gpui::CursorStyle::Crosshair,
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|event| event.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|event| event.position)
                    .or_else(|| released.map(|event| event.position))
                else {
                    return;
                };
                let free = moved
                    .map(|event| event.modifiers.platform)
                    .or_else(|| released.map(|event| event.modifiers.platform))
                    .unwrap_or(false);
                let _ = owner.update(cx, |view, cx| {
                    if view
                        .automation_view
                        .devices
                        .draft
                        .as_ref()
                        .is_none_or(|draft| draft.target != target)
                    {
                        return;
                    }
                    view.move_device_automation_pointer(
                        [f32::from(position.x), f32::from(position.y)],
                        free,
                    );
                    if released.is_some() {
                        if let Some(draft) = view.automation_view.devices.draft.take() {
                            view.commit_device_automation(draft.target, draft.points, cx);
                        }
                    }
                    cx.notify();
                    window.refresh();
                });
            },
            move |window, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view
                        .automation_view
                        .devices
                        .draft
                        .as_ref()
                        .is_some_and(|draft| draft.target == target)
                    {
                        view.automation_view.devices.draft = None;
                        cx.notify();
                        window.refresh();
                    }
                });
            },
            cx,
        ) {
            return false;
        }
        self.gain_lane.selected = None;
        self.automation_view.devices.selected = Some((target, value.beat));
        self.automation_view.devices.draft = Some(Draft {
            target,
            revision,
            points: candidate,
            initial: value,
            point: value,
            viewport,
            rect,
            origin: pos,
        });
        self.focus.focus(window);
        cx.notify();
        true
    }

    fn move_device_automation_pointer(&mut self, pos: [f32; 2], free: bool) {
        let Some(draft) = self.automation_view.devices.draft.as_ref() else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        if draft.revision != state.revision
            || draft
                .target
                .identity_current(&state.project, self.project_generation)
                .is_none()
        {
            self.automation_view.devices.draft = None;
            return;
        }
        let Some(entry) = self.automation_view.devices.entry(draft.target) else {
            return;
        };
        let Some(mut point) = pointer_value(
            &state.project,
            entry,
            self.snap_grid.with_platform_modifier(free),
            draft.viewport,
            draft.rect,
            pos,
        ) else {
            return;
        };
        if (pos[0] - draft.origin[0]).abs() < 2.0 {
            point.beat = draft.initial.beat;
        }
        if (pos[1] - draft.origin[1]).abs() < 2.0 {
            point.value = draft.initial.value;
        }
        point.outgoing = draft.point.outgoing;
        let mut points = draft.points.clone();
        points.retain(|old| old.beat != draft.point.beat);
        if points.iter().any(|old| old.beat == point.beat) {
            return;
        }
        let index = points.partition_point(|old| old.beat < point.beat);
        points.insert(index, point);
        let target = draft.target;
        if let Some(draft) = &mut self.automation_view.devices.draft {
            draft.points = points;
            draft.point = point;
        }
        self.automation_view.devices.selected = Some((target, point.beat));
    }
}

fn pointer_value(
    project: &Project,
    entry: &Entry,
    grid: super::super::musical_time::SnapGrid,
    viewport: Viewport,
    rect: [f32; 4],
    pos: [f32; 2],
) -> Option<AutomationPoint> {
    if rect
        .iter()
        .chain(pos.iter())
        .any(|value| !value.is_finite())
        || rect[2] <= 0.0
        || rect[3] <= 8.0
    {
        return None;
    }
    let frame = super::super::seek::pointer_to_frame(pos[0], rect[0], rect[2], viewport)?;
    let beat = grid.snap_beat(
        project,
        project.tempo_map().frame_to_beat(frame).ok()?,
        viewport,
        rect[2],
    )?;
    let value =
        entry.quantize((1.0 - (pos[1] - rect[1] - 4.0) / (rect[3] - 8.0)).clamp(0.0, 1.0))?;
    Some(AutomationPoint {
        beat,
        value: AutomationValue::new(value).ok()?,
        outgoing: AutomationInterpolation::Linear,
    })
}
