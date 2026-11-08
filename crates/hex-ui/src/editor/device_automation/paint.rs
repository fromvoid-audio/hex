//! Visible-lane composition and pointer projection; canonical edits stay on the
//! worker. Outer track list geometry includes lane space, never stretches clips.
use super::super::viewport::Viewport;
use super::*;
use crate::theme;
use gpui::{
    Bounds, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, canvas,
    div, point, prelude::*, px, rgb, size,
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
        let live = view.live_device_automation_base(&entry);
        // Readout: the held point, else a single selected point, else the base.
        let lanes = &view.automation_view.devices;
        let value = lanes
            .draft
            .as_ref()
            .filter(|draft| draft.target == target)
            .map(|draft| draft.point)
            .or_else(|| {
                lanes
                    .selected
                    .as_ref()
                    .filter(|selected| selected.target == target)
                    .and_then(|selected| match selected.beats.as_slice() {
                        [beat] => points.iter().find(|point| point.beat == *beat).copied(),
                        _ => None,
                    })
            })
            .map_or(live, |point| point.value.get());
        let overridden = view.device_automation_read_selection(target)
            == Some(hex_project::AutomationReadSelection::Base);
        let name = format!(
            "{} · {}{}",
            entry.label,
            entry.format(value),
            if overridden { " · overridden" } else { "" }
        );
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

fn recorded_spans(project: &Project, target: Target) -> &[hex_project::DeviceAutomationTouchSpan] {
    target
        .identity_current(project, target.generation)
        .and_then(|device| device.automation_target(target.kind))
        .map_or(&[], |lane| lane.touch.as_slice())
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
    // The paint closure re-borrows the displayed lane at paint time, so render
    // neither copies nor traverses its points (#4097).
    let dotted = recorded_spans(&state.project, target).is_empty()
        && view
            .automation_view
            .devices
            .points_ref(&state.project, target)
            .is_empty();
    let project = state.project.clone();
    let revision = state.revision;
    let base = view.live_device_automation_base(&entry);
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
                    let tempo = project.tempo_map();
                    let frame = |beat| tempo.beat_to_frame(beat).ok();
                    let x = |frame: u64| {
                        rect[0]
                            + ((frame as f64 - viewport.start as f64)
                                / viewport.length.max(1) as f64)
                                as f32
                                * rect[2]
                    };
                    let y = |value: f32| rect[1] + (1.0 - value) * (rect[3] - 8.0) + 4.0;
                    let xy = |beat: BeatTime, value: f32| -> Option<Point<Pixels>> {
                        Some(point(px(x(frame(beat)?)), px(y(value))))
                    };
                    let color = rgb(theme::color(theme::PaletteRole::Secondary));
                    let lanes = &entity.read(cx).automation_view.devices;
                    let all_points = lanes.points_ref(&project, target);
                    let paint_base = |section: std::ops::Range<u64>, window: &mut Window| {
                        let range = super::plot_geometry::visible_range(
                            all_points,
                            section.start,
                            section.end,
                            |point| frame(point.beat),
                        );
                        let points = all_points.get(range).unwrap_or_default();
                        if let Some(first) = points.first() {
                            let selected = lanes.selected.as_ref();
                            let paths = super::plot_geometry::reduce(
                                points.iter().filter_map(|value| {
                                    let position = xy(value.beat, value.value.get())?;
                                    Some(super::plot_geometry::PlotPoint {
                                        x: f32::from(position.x),
                                        y: f32::from(position.y),
                                        step: value.outgoing == AutomationInterpolation::Step,
                                        selected: selected.is_some_and(|selected| {
                                            selected.contains(target, value.beat)
                                        }),
                                    })
                                }),
                                entry.is_discrete(),
                                window.scale_factor(),
                            );
                            let mut path = PathBuilder::stroke(px(1.5));
                            path.move_to(point(
                                px(rect[0]),
                                px(rect[1] + (1.0 - first.value.get()) * (rect[3] - 8.0) + 4.0),
                            ));
                            for &(x, y) in &paths.line {
                                path.line_to(point(px(x), px(y)));
                            }
                            if let Some(&(_, y)) = paths.line.last() {
                                path.line_to(point(px(rect[0] + rect[2]), px(y)));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color);
                            }
                            for &(x, y, selected) in &paths.markers {
                                let position = point(px(x), px(y));
                                if !bounds.contains(&position) {
                                    continue;
                                }
                                let mut marker =
                                    PathBuilder::stroke(px(if selected { 3.0 } else { 2.0 }));
                                marker.move_to(point(position.x - px(2.0), position.y));
                                marker.line_to(point(position.x + px(2.0), position.y));
                                if let Ok(marker) = marker.build() {
                                    window.paint_path(marker, color);
                                }
                            }
                        } else {
                            let y = y(base);
                            let mut dash = ((x(section.start) - rect[0]) / 9.0).floor() * 9.0;
                            while dash < x(section.end) - rect[0] {
                                let mut path = PathBuilder::stroke(px(1.0));
                                path.move_to(point(px(rect[0] + dash), px(y)));
                                path.line_to(point(px(rect[0] + (dash + 5.0).min(rect[2])), px(y)));
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, color);
                                }
                                dash += 9.0;
                            }
                        }
                    };
                    let recorded = recorded_spans(&project, target);
                    let visible = viewport.start..viewport.end().unwrap_or(u64::MAX);
                    if recorded.is_empty() {
                        paint_base(visible, window);
                    } else {
                        super::plot_geometry::visible_sections(
                            recorded,
                            visible,
                            frame,
                            |section, span| {
                                let start = x(section.start);
                                let end = x(section.end);
                                if end <= start {
                                    return;
                                }
                                let clip = Bounds::new(
                                    point(px(start), bounds.origin.y),
                                    size(px(end - start), bounds.size.height),
                                );
                                window.with_content_mask(
                                    Some(gpui::ContentMask { bounds: clip }),
                                    |window| {
                                        let Some(span) = span else {
                                            paint_base(section, window);
                                            return;
                                        };
                                        let mut knots = super::plot_geometry::recorded_knots(
                                            span, section, frame,
                                        );
                                        let Some((at, value)) = knots.next() else {
                                            return;
                                        };
                                        let mut held = y(value.get());
                                        let mut path = PathBuilder::stroke(px(1.5));
                                        path.move_to(point(px(x(at)), px(held)));
                                        for (at, value) in knots {
                                            let at = px(x(at));
                                            path.line_to(point(at, px(held)));
                                            held = y(value.get());
                                            path.line_to(point(at, px(held)));
                                        }
                                        path.line_to(point(px(end), px(held)));
                                        if let Ok(path) = path.build() {
                                            window.paint_path(path, color);
                                        }
                                    },
                                );
                            },
                        );
                    }
                    {
                        let view = entity.read(cx);
                        let marquee =
                            view.automation_view
                                .devices
                                .marquee
                                .as_ref()
                                .filter(|marquee| {
                                    marquee.target == target && marquee.revision == revision
                                });
                        if let Some(marquee) = marquee.filter(|marquee| marquee.active) {
                            marquee.band.paint(
                                bounds,
                                theme::color(theme::PaletteRole::Secondary),
                                window,
                            );
                        }
                        if (marquee.is_some()
                            || view
                                .automation_view
                                .devices
                                .draft
                                .as_ref()
                                .is_some_and(|draft| {
                                    draft.target == target && draft.revision == revision
                                }))
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
        // Unpainted locator inside the same 4 px insets as the dotted stroke.
        .when(dotted, |plot| {
            plot.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .h(px(0.0))
                            .top(gpui::relative(1.0 - base.clamp(0.0, 1.0)))
                            .debug_selector(move || {
                                format!(
                                    "automation-dotted-{}-{}-{main}",
                                    target.device.value(),
                                    target.key()
                                )
                            }),
                    ),
            )
        })
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
            .state
            .as_ref()
            .is_some_and(|state| recorded_spans(&state.project, target).is_empty())
            && self
                .automation_view
                .devices
                .selected
                .as_ref()
                .is_some_and(|selected| selected.target == target)
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
        actions.extend(self.device_automation_read_action(target));
        if !main
            && self.device_panel.owner == Some(target.owner)
            && let Some(entry) = self.automation_view.devices.entry(target)
            && matches!(entry.metadata, EntryMetadata::HostGain)
        {
            actions.extend(self.native_automation_arm_action(entry));
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
        let lanes = &self.automation_view.devices;
        if self.edit_pending()
            || lanes.pending.is_some()
            || lanes.draft.is_some()
            || lanes.marquee.is_some()
            || !self.arrangement_view_change_available(cx)
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(device) = target.current(&state.project, self.project_generation) else {
            return false;
        };
        if device
            .automation_target(target.kind)
            .is_some_and(|lane| !lane.touch.is_empty())
        {
            self.error = Some("clear recorded automation before editing this lane".into());
            cx.notify();
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
                point_position(&state.project, viewport, rect, value)
                    .is_some_and(|[x, y]| (x - pos[0]).abs() <= 6.0 && (y - pos[1]).abs() <= 6.0)
            })
            .copied();
        let revision = state.revision;
        let previous = self.automation_view.devices.selected.clone();
        if let Some(hit) = hit {
            // Pressing a selected point (or Shift) moves the whole selection.
            let mut beats = previous
                .as_ref()
                .map(|selected| selected.beats_in(target).to_vec())
                .unwrap_or_default();
            let grouped = event.modifiers.shift || beats.contains(&hit.beat);
            if !grouped {
                beats.clear();
            }
            beats.push(hit.beat);
            let Some(selection) = Selection::new(target, beats) else {
                return false;
            };
            let (group, rest) = points
                .iter()
                .partition(|point| selection.contains(target, point.beat));
            let collapse = !event.modifiers.shift && selection.beats.len() > 1;
            if !self.capture_device_automation_pointer(target, window, cx) {
                return false;
            }
            self.automation_view.devices.selected = Some(selection);
            self.automation_view.devices.draft = Some(Draft {
                target,
                revision,
                points,
                group,
                rest,
                collapse,
                initial: hit,
                point: hit,
                viewport,
                rect,
                origin: pos,
            });
        } else {
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
            let mut click = points;
            match click.binary_search_by_key(&value.beat, |point| point.beat) {
                Ok(index) => click[index] = value,
                Err(index) => click.insert(index, value),
            }
            if click.len() > hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE {
                return false;
            }
            // Adding points comes first: a plain blank press inserts the point
            // and drags it, as before marquee selection; only Shift draws a band.
            if !hex_widgets::MarqueeMode::starts(event.modifiers) {
                if !self.capture_device_automation_pointer(target, window, cx) {
                    return false;
                }
                let rest = click
                    .iter()
                    .copied()
                    .filter(|point| point.beat != value.beat)
                    .collect();
                self.automation_view.devices.selected = Some(Selection::one(target, value.beat));
                self.automation_view.devices.draft = Some(Draft {
                    target,
                    revision,
                    points: click,
                    group: vec![value],
                    rest,
                    collapse: false,
                    initial: value,
                    point: value,
                    viewport,
                    rect,
                    origin: pos,
                });
                self.gain_lane.selected = None;
                self.focus.focus(window);
                cx.notify();
                return true;
            }
            let Some(band) = hex_widgets::Marquee::new(pos) else {
                return false;
            };
            if !self.capture_device_automation_pointer(target, window, cx) {
                return false;
            }
            self.automation_view.devices.marquee = Some(MarqueeDraft {
                target,
                revision,
                band,
                mode: hex_widgets::MarqueeMode::from_modifiers(event.modifiers),
                base: previous,
                click,
                click_beat: value.beat,
                viewport,
                rect,
                active: false,
            });
        }
        self.gain_lane.selected = None;
        self.focus.focus(window);
        cx.notify();
        true
    }

    fn capture_device_automation_pointer(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
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
                    if view.device_automation_pointer(
                        target,
                        [f32::from(position.x), f32::from(position.y)],
                        free,
                        released.is_some(),
                        cx,
                    ) {
                        cx.notify();
                        window.refresh();
                    }
                });
            },
            move |window, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    let lanes = &mut view.automation_view.devices;
                    let mut changed = false;
                    if lanes
                        .draft
                        .as_ref()
                        .is_some_and(|draft| draft.target == target)
                    {
                        lanes.draft = None;
                        changed = true;
                    }
                    if lanes
                        .marquee
                        .as_ref()
                        .is_some_and(|marquee| marquee.target == target)
                        && let Some(marquee) = lanes.marquee.take()
                    {
                        lanes.selected = marquee.base;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                        window.refresh();
                    }
                });
            },
            cx,
        )
    }

    /// Routes one captured move/release to the held point drag or blank press.
    fn device_automation_pointer(
        &mut self,
        target: Target,
        pos: [f32; 2],
        free: bool,
        released: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let lanes = &self.automation_view.devices;
        if lanes
            .marquee
            .as_ref()
            .is_some_and(|marquee| marquee.target == target)
        {
            self.move_device_automation_marquee(pos);
            if released
                && let Some(marquee) = self.automation_view.devices.marquee.take()
                && !marquee.active
            {
                // Live: a blank click adds a point; only a drag selects.
                self.automation_view.devices.selected =
                    Some(Selection::one(target, marquee.click_beat));
                self.commit_device_automation(target, marquee.click, cx);
            }
            return true;
        }
        if lanes
            .draft
            .as_ref()
            .is_none_or(|draft| draft.target != target)
        {
            return false;
        }
        self.move_device_automation_pointer(pos, free);
        if released && let Some(draft) = self.automation_view.devices.draft.take() {
            if draft.collapse && draft.point == draft.initial {
                self.automation_view.devices.selected =
                    Some(Selection::one(target, draft.initial.beat));
            }
            self.commit_device_automation(draft.target, draft.points, cx);
        }
        true
    }

    fn move_device_automation_marquee(&mut self, pos: [f32; 2]) {
        let Some(state) = &self.state else {
            return;
        };
        let lanes = &mut self.automation_view.devices;
        let Some(marquee) = lanes.marquee.as_mut() else {
            return;
        };
        if marquee.revision != state.revision {
            lanes.marquee = None;
            return;
        }
        marquee.band.update(pos);
        marquee.active |= marquee.band.crossed_threshold();
        if !marquee.active {
            return;
        }
        let marquee = marquee.clone();
        let points = lanes.points(state, marquee.target);
        let hits = marquee.band.hits(points.iter().filter_map(|point| {
            point_position(&state.project, marquee.viewport, marquee.rect, point)
                .map(|position| (point.beat, position))
        }));
        let base = marquee
            .base
            .as_ref()
            .map_or(&[][..], |base| base.beats_in(marquee.target));
        lanes.selected = Selection::new(marquee.target, marquee.mode.combine(base, &hits));
    }

    /// Moves the pressed point to the pointer and the rest of its group by the
    /// same beat/value delta. A move that would collide with an unselected
    /// point or cross beat zero keeps the previous candidate.
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
        point.outgoing = draft.initial.outgoing;
        let beat_delta = i128::from(point.beat.raw()) - i128::from(draft.initial.beat.raw());
        let value_delta = point.value.get() - draft.initial.value.get();
        let mut moved = Vec::with_capacity(draft.group.len());
        for original in &draft.group {
            if original.beat == draft.initial.beat {
                moved.push(point);
                continue;
            }
            let Some(beat) = u64::try_from(i128::from(original.beat.raw()) + beat_delta)
                .ok()
                .map(BeatTime::from_raw)
            else {
                return;
            };
            let Some(value) = entry
                .quantize((original.value.get() + value_delta).clamp(0.0, 1.0))
                .and_then(|value| AutomationValue::new(value).ok())
            else {
                return;
            };
            moved.push(AutomationPoint {
                beat,
                value,
                outgoing: original.outgoing,
            });
        }
        if moved
            .iter()
            .any(|moved| draft.rest.iter().any(|rest| rest.beat == moved.beat))
        {
            return;
        }
        let mut points = draft.rest.clone();
        points.extend_from_slice(&moved);
        points.sort_unstable_by_key(|point| point.beat);
        let target = draft.target;
        if let Some(draft) = &mut self.automation_view.devices.draft {
            draft.points = points;
            draft.point = point;
        }
        self.automation_view.devices.selected =
            Selection::new(target, moved.iter().map(|point| point.beat).collect());
    }
}

/// Pixel center of a painted point, or `None` outside the viewport.
fn point_position(
    project: &Project,
    viewport: Viewport,
    rect: [f32; 4],
    point: &AutomationPoint,
) -> Option<[f32; 2]> {
    let x = viewport.fraction(project.tempo_map().beat_to_frame(point.beat).ok()?)?;
    Some([
        rect[0] + x * rect[2],
        rect[1] + (1.0 - point.value.get()) * (rect[3] - 8.0) + 4.0,
    ])
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
