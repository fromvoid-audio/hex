use super::super::waveform;
use super::*;
use crate::diagnostics::ActionTraceContext as _;
use crate::theme;
use gpui::{Context, Div, MouseButton, canvas, div, prelude::*, px, relative, rgb};
use hex_widgets::{RANGE_HANDLE_FRACTION, RangeSelectionTarget, RangeSelectionZones};

const EVENT_RULER_HEIGHT: f32 = super::super::timeline_strip::RULER_HEIGHT;

pub(in crate::editor) fn view(
    view: &mut EditorView,
    render_window: &gpui::Window,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    let (clip_id, root, _) = view.event_context()?;
    view.sync_audio_navigation();
    let owner = (clip_id, view.project_generation);
    let state = view.state.as_ref()?;
    let clip = state.project.clip(clip_id)?.clone();
    let events = match clip.editable_audio_events() {
        Ok(events) => events,
        Err(error) => {
            return Some(
                div()
                    .id("audio-event-editor-limit")
                    .flex_1()
                    .text_xs()
                    .child(theme::text(error.0)),
            );
        }
    };
    if view.audio_event_editor.selected.is_some_and(|id| {
        !events.events.iter().any(|e| {
            e.id == id && e.start >= editor_bounds(&root).0 && e.end <= editor_bounds(&root).1
        })
    }) {
        view.audio_event_editor.selected = None;
        view.audio_event_editor.retained_selection = None;
    }
    let pending = view
        .audio_event_editor
        .pending
        .as_ref()
        .filter(|p| p.owner == owner);
    let operation = pending.map(|p| &p.operation).or_else(|| {
        view.audio_event_editor
            .gesture
            .as_ref()
            .and_then(|g| g.operation.as_ref())
    });
    let edited = pending.map(|p| p.preview.clone()).or_else(|| {
        operation.and_then(|op| events.edited(op.clone(), root.domain.clock.length()).ok())
    });
    let intersections = edited
        .as_ref()
        .and(operation)
        .map(|op| paint::replacement_spans(&events, op))
        .unwrap_or_default();
    let preview = edited.unwrap_or_else(|| events.clone());
    let local = view.event_viewport(&root);
    let viewport = view.event_project_viewport(&root)?;
    let first = root
        .root_frame_at(editor_bounds(&root).0, state.project.tempo_map())
        .ok()?;
    let frame_at = |position: u64| root.root_frame_at(position, state.project.tempo_map()).ok();
    let frame_to_x =
        |frame: u64| frame.saturating_sub(first + local.start) as f32 / local.length.max(1) as f32;
    let width = view
        .audio_event_editor
        .bounds
        .map_or(640.0, |bounds| f32::from(bounds.size.width));
    let mut body = div()
        .id("audio-events-waveform")
        .on_scroll_wheel(cx.listener(|view, event, window, cx| {
            if view.audio_navigation_wheel(event, false, window, cx) {
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .debug_selector(|| "audio-events-waveform".into())
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .cursor(gpui::CursorStyle::Arrow);
    let grid = super::super::musical_time::grid_lines(
        &state.project,
        viewport,
        width,
        view.audio_event_editor.grid,
    );
    for line in &grid {
        let x = (line.frame - viewport.start) as f32 / viewport.length.max(1) as f32;
        body = body.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(x))
                .w(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Grid))),
        );
    }
    let mut ruler = super::super::timeline_strip::ruler_surface()
        .debug_selector(|| "audio-events-time-ruler".into())
        .absolute()
        .top_0()
        .left_0()
        .right_0();
    for tick in super::super::musical_time::ruler_ticks(&state.project, viewport, width) {
        let x = (tick.frame - viewport.start) as f32 / viewport.length.max(1) as f32;
        ruler = ruler.children(super::super::timeline_strip::ruler_tick(
            x,
            tick.label,
            tick.bar_start,
            tick.meter_marker,
            tick.fragmentary_bar,
        ));
    }
    body = body.children([ruler]);
    for event in &preview.events {
        let raw_a = event.start.max(editor_bounds(&root).0);
        let raw_b = event.end.min(editor_bounds(&root).1);
        if raw_a >= raw_b {
            continue;
        }
        let start = frame_at(raw_a)?;
        let end = frame_at(raw_b)?;
        let visible_start = start.max(first + local.start);
        let visible_end = end.min(first + local.start + local.length);
        if visible_start >= visible_end {
            continue;
        }
        let event_id = event.id;
        let event_active = event.active;
        let selected = view.audio_event_editor.selected == Some(event.id);
        let mut element = div()
            .id(("audio-event", event.id))
            .debug_selector(move || format!("audio-event-{}", event_id))
            .absolute()
            .top(px(25.0))
            .bottom(px(3.0))
            .left(relative(frame_to_x(visible_start)))
            .w(relative(
                (visible_end - visible_start) as f32 / local.length.max(1) as f32,
            ))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(theme::color(if selected {
                theme::PaletteRole::Secondary
            } else {
                theme::PaletteRole::Border
            })))
            .bg(rgb(theme::color(theme::PaletteRole::Clip)))
            .opacity(if event.active { 1.0 } else { 0.5 })
            .cursor(gpui::CursorStyle::Arrow);
        let handle_height = view
            .audio_event_editor
            .bounds
            .and_then(|bounds| {
                RangeSelectionZones::new(f32::from(bounds.size.height) - EVENT_RULER_HEIGHT, 0.0)
            })
            .map_or(0.0, |zones| (zones.handle_height() - 3.0).max(0.0));
        element = element.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(handle_height))
                .cursor(gpui::CursorStyle::OpenHand),
        );
        if let Some(pasted) = view
            .audio_event_editor
            .pending
            .as_ref()
            .filter(|p| p.owner == owner)
            .and_then(|p| p.waveform.as_ref())
            .filter(|p| event.start >= p.start && event.end <= p.end)
        {
            element = element.child(
                waveform::waveform_source_segment_gain_view_cached(
                    std::sync::Arc::clone(&pasted.waveform),
                    0,
                    visible_start.saturating_sub(pasted.frame_start),
                    visible_end.saturating_sub(pasted.frame_start),
                    clip.gain(),
                    true,
                    std::rc::Rc::clone(&view.waveform_geometry_cache),
                )
                .debug_selector(move || format!("audio-event-waveform-{event_id}")),
            );
        } else if let Some(original) =
            events.events.iter().find(|e| e.id == event.id).or_else(|| {
                events.events.iter().find(|e| {
                    e.source_start <= event.source_start
                        && event.source_start < e.source_start + e.length()
                })
            })
        {
            let source_raw = prepared_waveform_position(*event, *original, raw_a)?;
            let source_root = frame_at(source_raw)?;
            let source_frame = clip
                .start()
                .checked_add(source_root.saturating_sub(first))?
                .checked_add(visible_start - start)?;
            let end_frame = source_frame.checked_add(visible_end - visible_start)?;
            if let Some(prepared) = state.audio_clip_waveforms.get(&clip_id) {
                element = element.child(
                    waveform::waveform_source_segment_gain_view_cached(
                        std::sync::Arc::clone(&prepared.waveform),
                        0,
                        source_root + (visible_start - start),
                        source_root + (visible_end - start),
                        clip.gain(),
                        true,
                        std::rc::Rc::clone(&view.waveform_geometry_cache),
                    )
                    .debug_selector(move || format!("audio-event-waveform-{event_id}")),
                );
            } else if events.sources.is_empty()
                && let Some(wave) = state.waveforms.get(&clip.asset())
            {
                element = element.child(
                    waveform::waveform_view_cached(
                        wave,
                        &clip,
                        source_frame,
                        end_frame,
                        std::rc::Rc::clone(&view.waveform_geometry_cache),
                    )
                    .debug_selector(move || format!("audio-event-waveform-{event_id}")),
                );
            }
        }
        element = element.child(
            div()
                .debug_selector(move || {
                    format!(
                        "audio-event-{}-{}",
                        event_id,
                        if event_active { "active" } else { "inactive" }
                    )
                })
                .absolute()
                .top_0()
                .left(px(3.0))
                .text_xs()
                .text_color(rgb(theme::color(if event.active {
                    theme::PaletteRole::Text
                } else {
                    theme::PaletteRole::Muted
                })))
                .child(theme::text(if event.active {
                    format!("event {}", event.id)
                } else {
                    format!("inactive · event {}", event.id)
                })),
        );
        if event.start >= editor_bounds(&root).0 && start >= first + local.start {
            element = element.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(gpui::CursorStyle::ResizeLeftBracket),
            );
        }
        if event.end <= editor_bounds(&root).1 && end <= first + local.start + local.length {
            element = element.child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(gpui::CursorStyle::ResizeRightBracket),
            );
        }
        let retained_gain = view
            .audio_event_editor
            .retained_selection
            .as_ref()
            .filter(|(path, _)| {
                view.audio_event_editor.selected == Some(event_id)
                    && path.first() == Some(&event.source)
            })
            .and_then(|(path, id)| {
                preview
                    .sources
                    .iter()
                    .find(|source| Some(&source.id) == path.first())
                    .and_then(|source| super::retained_events(&source.clip, &path[1..]))
                    .and_then(|events| events.events.into_iter().find(|inner| inner.id == *id))
            })
            .map(|inner| inner.gain);
        let display_gain = retained_gain.unwrap_or(event.gain);
        if matches!(operation, Some(AudioEventEdit::SetGain { event: id, .. }) if *id == event_id)
            || (retained_gain.is_some()
                && matches!(operation, Some(AudioEventEdit::SetRetainedGain { .. })))
        {
            element = element.child(
                div()
                    .debug_selector(move || format!("audio-event-gain-value-{event_id}"))
                    .absolute()
                    .top(px(5.0))
                    .right(px(7.0))
                    .text_xs()
                    .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                    .child(theme::text(format!(
                        "{:+.1} db",
                        f64::from(display_gain.millidb()) / 1000.0
                    ))),
            );
        }
        if !view.audio_event_editor.fade_key_held && !view.time_selection_key_held {
            let gain = display_gain.millidb();
            element = element.child(
                div()
                    .id(("audio-event-gain", event_id))
                    .debug_selector(move || format!("audio-event-gain-{event_id}"))
                    .absolute()
                    .top_0()
                    .left(px(7.0))
                    .right(px(7.0))
                    .h(px(4.0))
                    .cursor(gpui::CursorStyle::ResizeUpDown)
                    .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Secondary))))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::super::waveform::ClipTooltip {
                            kind: crate::tooltips::Kind::control("event gain"),
                            name: format!(
                                "gain {:+.1} db · drag down to reduce · shift for fine",
                                f64::from(gain) / 1000.0
                            ),
                        })
                        .into()
                    }),
            );
        }
        element = element.children(view.event_fade_elements(
            &root,
            *event,
            visible_start,
            visible_end,
            local.length,
            cx,
        ));
        body = body.child(element);
    }
    // One bounded grid above the opaque event bodies, using the same light
    // painter as Arrangement. It has no hit handlers and retains edge cursors.
    body = body.children(
        super::super::tracks::clip_grid_layer(&grid, viewport.start, viewport.end()?).map(|grid| {
            grid.debug_selector(|| "audio-event-musical-grid".into())
                .top(px(25.0))
                .bottom(px(3.0))
        }),
    );
    for (index, (a, b)) in intersections.into_iter().enumerate() {
        let a = frame_at(a.max(editor_bounds(&root).0))?.max(first + local.start);
        let b = frame_at(b.min(editor_bounds(&root).1))?.min(first + local.start + local.length);
        if a < b {
            body = body.child(
                div()
                    .debug_selector(move || format!("audio-event-overlap-{index}"))
                    .absolute()
                    .top(px(25.0))
                    .bottom(px(3.0))
                    .left(relative(frame_to_x(a)))
                    .w(relative((b - a) as f32 / local.length.max(1) as f32))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(0.72),
            );
        }
    }
    let selected_range = match view
        .audio_event_editor
        .gesture
        .as_ref()
        .and_then(|g| g.operation.as_ref())
    {
        Some(AudioEventEdit::MoveRange {
            start,
            end,
            destination,
        }) => Some((*destination, *destination + (*end - *start))),
        _ => view.audio_event_editor.range,
    };
    if let Some((a, b)) = selected_range.filter(|(a, b)| a < b) {
        let a = frame_at(a)?.max(first + local.start);
        let b = frame_at(b)?.min(first + local.start + local.length);
        if a < b {
            body = body.child(
                div()
                    .debug_selector(|| "audio-event-time-selection".into())
                    .absolute()
                    .top(px(EVENT_RULER_HEIGHT))
                    .bottom_0()
                    .left(relative(frame_to_x(a)))
                    .w(relative((b - a) as f32 / local.length.max(1) as f32))
                    .bg(rgb(theme::time_selection_fill()))
                    .opacity(theme::TIME_SELECTION_OPACITY)
                    .child(
                        div()
                            .id("audio-event-range-handle")
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(relative(RANGE_HANDLE_FRACTION))
                            .debug_selector(|| "audio-event-range-handle".into())
                            .cursor(gpui::CursorStyle::OpenHand)
                            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Text)))),
                    ),
            );
        }
    }
    let cursor = frame_at(
        view.audio_event_editor
            .cursor
            .min(root.domain.clock.length()),
    )?;
    if (first + local.start..=first + local.start + local.length).contains(&cursor) {
        body = body.child(
            div()
                .debug_selector(|| "audio-event-cursor".into())
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(frame_to_x(cursor)))
                .w(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Primary))),
        );
    }
    body = body.children(first.checked_add(local.start).and_then(|start| {
        view.audio_play_start_marker(
            &root,
            Viewport {
                start,
                length: local.length,
            },
            render_window,
            cx,
        )
    }));
    if let Some(position) = view
        .audio_playhead_root_position()
        .and_then(frame_at)
        .filter(|frame| (first + local.start..first + local.start + local.length).contains(frame))
    {
        body = body.child(
            div()
                .debug_selector(|| "audio-playback-position".into())
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(frame_to_x(position)))
                .w(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    let entity = cx.entity();
    body = body.child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                entity.update(cx, |view, cx| {
                    let changed = view.audio_event_editor.bounds != Some(bounds);
                    view.audio_event_editor.bounds = Some(bounds);
                    if changed {
                        view.audio_event_editor.gesture = None;
                        cx.notify();
                    }
                });
                // Existing geometry tracking above may end a stale gesture;
                // the lease witness itself only reads current held state.
                let view = entity.read(cx);
                if let Some(gesture) = view.audio_event_editor.gesture.as_ref()
                    && view.audio_event_focus.is_focused(window)
                {
                    window.renew_pointer_capture(
                        entity.entity_id(),
                        view.audio_event_pointer_cursor(gesture.kind),
                    );
                }
            },
        )
        .absolute()
        .inset_0(),
    );
    body = body.on_mouse_down(
        MouseButton::Left,
        cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
            if view.edit_pending() || !view.event_context_current() {
                return;
            }
            let Some(position) =
                view.event_pointer_position(f32::from(event.position.x), event.modifiers.platform)
            else {
                return;
            };
            let Some(raw) = view.event_pointer_position(
                f32::from(event.position.x),
                view.audio_event_editor.grid != SnapGrid::Off,
            ) else {
                return;
            };
            view.advance_selection_intent();
            view.audio_event_focus.focus(window);
            let Some((clip_id, root, _)) = view.event_context() else {
                return;
            };
            let Some(state) = view.state.as_ref() else {
                return;
            };
            let Some(clip) = state.project.clip(clip_id) else {
                return;
            };
            let Some(bounds) = view.audio_event_editor.bounds else {
                return;
            };
            let local_y = f32::from(event.position.y - bounds.origin.y);
            let ruler = (0.0..EVENT_RULER_HEIGHT).contains(&local_y);
            let target = if ruler {
                RangeSelectionTarget::Paint
            } else {
                let Some(target) = RangeSelectionZones::new(
                    f32::from(bounds.size.height) - EVENT_RULER_HEIGHT,
                    0.0,
                )
                .and_then(|zones| {
                    zones.target(local_y - EVENT_RULER_HEIGHT, true, event.modifiers.shift)
                }) else {
                    return;
                };
                target
            };
            let Ok(events) = clip.editable_audio_events() else {
                return;
            };
            let hit = events
                .events
                .iter()
                .find(|e| e.start <= raw && raw < e.end)
                .copied();
            let range_hit = view
                .audio_event_editor
                .range
                .filter(|(a, b)| a <= &raw && raw < *b);
            let kind = if view.time_selection_key_held {
                Kind::Select
            } else if target == RangeSelectionTarget::Move && range_hit.is_some() {
                if let Some((start, end)) = range_hit {
                    Kind::RangeMove { start, end }
                } else {
                    Kind::Select
                }
            } else if ruler || event.modifiers.shift || hit.is_none() {
                Kind::Select
            } else if let Some(hit) = hit {
                let first = root
                    .root_frame_at(editor_bounds(&root).0, state.project.tempo_map())
                    .unwrap_or(0);
                let local = view.event_viewport(&root);
                let bounds = view.audio_event_editor.bounds;
                let width = bounds.map_or(1.0, |b| f32::from(b.size.width));
                let offset = bounds.map_or(0.0, |b| f32::from(event.position.x - b.origin.x));
                let x = |raw: u64| {
                    root.root_frame_at(raw, state.project.tempo_map())
                        .unwrap_or(0)
                        .saturating_sub(first + local.start) as f32
                        / local.length.max(1) as f32
                        * width
                };
                if hit.start >= editor_bounds(&root).0
                    && root
                        .root_frame_at(hit.start, state.project.tempo_map())
                        .is_ok_and(|frame| frame >= first + local.start)
                    && (offset - x(hit.start)).abs() <= 6.0
                {
                    Kind::Leading(hit)
                } else if hit.end <= editor_bounds(&root).1
                    && root
                        .root_frame_at(hit.end, state.project.tempo_map())
                        .is_ok_and(|frame| frame <= first + local.start + local.length)
                    && (offset - x(hit.end)).abs() <= 6.0
                {
                    Kind::Trailing(hit)
                } else if !view.audio_event_editor.fade_key_held
                    && (EVENT_RULER_HEIGHT..EVENT_RULER_HEIGHT + 5.0).contains(&local_y)
                {
                    let target = if let Some((path, id)) = view
                        .audio_event_editor
                        .retained_selection
                        .as_ref()
                        .filter(|(path, _)| {
                            path.first() == Some(&hit.source)
                                && view.audio_event_editor.selected == Some(hit.id)
                        }) {
                        let Some(inner) = super::retained_events(clip, path).and_then(|events| {
                            events.events.into_iter().find(|event| event.id == *id)
                        }) else {
                            return;
                        };
                        inner
                    } else {
                        hit
                    };
                    Kind::Gain {
                        event: target,
                        origin_y: f32::from(event.position.y),
                        last_y: f32::from(event.position.y),
                        value: f64::from(target.gain.millidb()),
                    }
                } else if target == RangeSelectionTarget::Move {
                    Kind::Move(hit)
                } else {
                    Kind::Select
                }
            } else {
                Kind::Select
            };
            if !view.capture_audio_event_pointer(kind, window, cx) {
                return;
            }
            if !matches!(kind, Kind::Gain { .. })
                || view.audio_event_editor.selected != hit.map(|event| event.id)
                || view
                    .audio_event_editor
                    .retained_selection
                    .as_ref()
                    .is_some_and(|(path, _)| {
                        hit.is_none_or(|event| path.first() != Some(&event.source))
                    })
            {
                view.audio_event_editor.retained_selection = None;
            }
            if matches!(kind, Kind::Select) {
                view.audio_event_editor.range = Some((position, position));
                view.audio_event_editor.selected = None;
            } else if !matches!(kind, Kind::RangeMove { .. }) {
                view.audio_event_editor.range = None;
                view.audio_event_editor.selected = hit.map(|event| event.id);
            }
            view.audio_event_editor.cursor = position;
            view.audio_event_editor.gesture = Some(Gesture {
                pointer_x: f32::from(event.position.x),
                origin_x: f32::from(event.position.x),
                moved: false,
                anchor: if matches!(kind, Kind::Select) {
                    position
                } else {
                    raw
                },
                kind,
                operation: None,
            });
            cx.stop_propagation();
            cx.notify();
        }),
    );

    let retained = view
        .audio_event_editor
        .selected
        .and_then(|id| preview.events.iter().find(|event| event.id == id))
        .and_then(|event| preview.sources.iter().find(|root| root.id == event.source));
    let retained_label = view
        .audio_event_editor
        .retained_selection
        .as_ref()
        .and_then(|(path, id)| {
            let root = retained.filter(|root| Some(&root.id) == path.first())?;
            let events = super::retained_events(&root.clip, &path[1..])?;
            let event = events.events.iter().find(|event| event.id == *id)?;
            Some(format!(
                "source level {} · event {} · {}",
                path.len(),
                id,
                if event.active { "active" } else { "inactive" }
            ))
        });
    let retained_controls = retained.map(|_| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                waveform::button(
                    "src",
                    "select retained source events · 0 toggles activation",
                    ("audio-retained-source", 0),
                )
                .debug_selector(|| "audio-retained-source".into())
                .on_click(cx.listener(|view, _, window, cx| {
                    view.audio_event_focus.focus(window);
                    if view.activation_gesture_blocked()
                        || view.audio_event_editor.pending.is_some()
                        || view.audio_event_editor.gesture.is_some()
                    {
                        return;
                    }
                    if view.audio_event_editor.retained_selection.is_some() {
                        view.audio_event_editor.retained_selection = None;
                    } else {
                        view.event_retained_step(true);
                    }
                    cx.notify();
                })),
            )
            .children(retained_label.as_ref().map(|label| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        waveform::button("↑", "parent retained source", ("audio-retained-up", 0))
                            .debug_selector(|| "audio-retained-up".into())
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.audio_event_focus.focus(window);
                                view.event_retained_depth(false);
                                cx.notify();
                            })),
                    )
                    .child(
                        waveform::button(
                            "‹",
                            "previous retained source event",
                            ("audio-retained-prev", 0),
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.audio_event_focus.focus(window);
                            view.event_retained_step(false);
                            cx.notify();
                        })),
                    )
                    .child(
                        waveform::button("↓", "open retained source", ("audio-retained-down", 0))
                            .debug_selector(|| "audio-retained-down".into())
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.audio_event_focus.focus(window);
                                view.event_retained_depth(true);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
                            .child(theme::text(label.clone())),
                    )
                    .child(
                        waveform::button(
                            "›",
                            "next retained source event",
                            ("audio-retained-next", 0),
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.audio_event_focus.focus(window);
                            view.event_retained_step(true);
                            cx.notify();
                        })),
                    )
            }))
    });
    let grid = view.audio_event_editor.grid;
    let toolbar = div()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .child(
            waveform::button("✂", "split audio event · e", ("audio-event-split", 0)).on_click(
                cx.listener(|view, _, window, cx| {
                    view.audio_event_focus.focus(window);
                    view.event_split();
                    cx.notify();
                }),
            ),
        )
        .child(
            waveform::button(
                "−",
                "delete audio event or time range",
                ("audio-event-delete", 0),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.audio_event_focus.focus(window);
                view.event_delete();
                cx.notify();
            })),
        )
        .child(
            waveform::button(
                grid.label(),
                "event grid · cmd+4 toggle · hold cmd to invert snap",
                ("audio-event-grid", 0),
            )
            .w(px(72.0))
            .on_click(cx.listener(|view, _, window, cx| {
                view.audio_event_focus.focus(window);
                view.audio_event_editor.grid = view.audio_event_editor.grid.toggled();
                view.audio_event_editor.gesture = None;
                cx.notify();
            })),
        )
        .child(
            waveform::button(
                "+",
                "zoom audio events · cmd+plus",
                ("audio-event-zoom-in", 0),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.audio_event_focus.focus(window);
                view.event_zoom(true);
                cx.notify();
            })),
        )
        .child(
            waveform::button(
                "−",
                "zoom out audio events · cmd+minus",
                ("audio-event-zoom-out", 0),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.audio_event_focus.focus(window);
                view.event_zoom(false);
                cx.notify();
            })),
        )
        .child(view.zoom_fit_button(super::super::zoom_fit::Target::Audio, false, cx))
        .child(view.zoom_fit_button(super::super::zoom_fit::Target::Audio, true, cx))
        .children(retained_controls)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(
                    "drag ruler for time · drag event to move · cmd for free",
                )),
        );
    let body = body.children(view.time_selection_cursor_layer());
    let root = div()
        .id("audio-event-editor")
        .debug_selector(|| "audio-event-editor".into())
        .track_focus(&view.audio_event_focus)
        .key_context("hex_audio_events")
        .min_w_0()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(toolbar)
        .children(view.audio_loop_ruler(cx))
        .child(body)
        .on_action(cx.trace(|view, _: &super::super::ToggleSnapGrid, _, cx| {
            view.audio_event_editor.grid = if view.audio_event_editor.grid == SnapGrid::Off {
                SnapGrid::Adaptive
            } else {
                SnapGrid::Off
            };
            view.audio_event_editor.gesture = None;
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::MoveLeft, window, cx| {
            view.event_nudge(false, false, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::MoveRight, window, cx| {
            view.event_nudge(true, false, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::TrimLeft, window, cx| {
            view.event_nudge(false, true, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::TrimRight, window, cx| {
            view.event_nudge(true, true, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::DuplicateClip, _, cx| {
            view.event_duplicate();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.trace(|view, _: &super::super::SelectAllMidiNotes, _, cx| {
                view.event_select_all();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.trace(|view, _: &super::super::InsertTimeSelection, _, cx| {
                view.event_paste(true);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.trace(|view, _: &super::super::CopyTimeSelection, _, cx| {
                view.event_copy(false);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.trace(|view, _: &super::super::CutTimeSelection, _, cx| {
            view.event_copy(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.trace(|view, _: &super::super::PasteTimeSelection, _, cx| {
                view.event_paste(false);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.trace(|view, _: &super::super::DeleteClip, _, cx| {
            if !view.selection_delete_blocked() {
                view.event_delete();
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::SplitClip, _, cx| {
            view.event_split();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::ZoomIn, _, cx| {
            view.event_zoom(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::ZoomOut, _, cx| {
            view.event_zoom(false);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::FitTimeline, _, cx| {
            view.audio_fit_all();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::PanTimelineLeft, _, cx| {
            view.event_pan(false);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::PanTimelineRight, _, cx| {
            view.event_pan(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.trace(|view, _: &super::super::CancelDragOrStop, _, cx| {
            if !view.cancel_audio_loop_gesture() {
                view.advance_selection_intent();
                view.audio_event_editor.gesture = None;
                view.audio_event_editor.range = None;
            }
            cx.stop_propagation();
            cx.notify();
        }));
    Some(root)
}

// The sidecar is the accepted, event-composed root (not the original asset).
// Preview pieces therefore refer back to the current event's destination there.
pub(super) fn prepared_waveform_position(
    event: AudioEvent,
    original: AudioEvent,
    visible: u64,
) -> Option<u64> {
    original
        .start
        .checked_add(event.source_start.saturating_sub(original.source_start))?
        .checked_add(visible.checked_sub(event.start)?)
}
