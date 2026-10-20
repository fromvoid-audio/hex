use super::super::waveform;
use super::*;
use crate::theme;
use gpui::{Context, Div, MouseButton, canvas, div, prelude::*, px, relative, rgb};

pub(in crate::editor) fn view(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    let data = match view.event_data() {
        Ok(Some(data)) => data,
        Ok(None) => return None,
        Err(ReadError::Model(error)) => {
            view.error = Some(error.0.into());
            return Some(
                div()
                    .id("audio-event-editor-limit")
                    .flex_1()
                    .text_xs()
                    .child(theme::text(error.0)),
            );
        }
        Err(error) => {
            view.event_error(error);
            return None;
        }
    };
    let clip_id = data.clip;
    let root = data.root;
    let revision = data.revision;
    let source_key = data.source_key;
    let owner = (clip_id, view.project_generation);
    let same_context = view.audio_event_editor.owner == Some(owner)
        && view.audio_event_editor.revision == revision
        && view.audio_event_editor.source_key == Some(source_key);
    let events = data.events;
    let preview = if same_context {
        view.audio_event_editor
            .gesture
            .as_ref()
            .and_then(|g| g.operation.clone())
            .and_then(|op| events.edited(op, root.domain.clock.length()).ok())
            .unwrap_or_else(|| events.clone())
    } else {
        events.clone()
    };
    let local = match view.event_viewport(&root) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(error) => {
            view.event_error(error);
            return None;
        }
    };
    let viewport = match view.event_project_viewport(&root) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(error) => {
            view.event_error(error);
            return None;
        }
    };
    let width = view
        .audio_event_editor
        .bounds
        .map_or(640.0, |bounds| f32::from(bounds.size.width));
    let same_owner = view.audio_event_editor.owner == Some(owner);
    let grid = if same_owner {
        view.audio_event_editor.grid
    } else {
        SnapGrid::default()
    };
    let cursor = if same_owner {
        view.audio_event_editor
            .cursor
            .clamp(root.view_start, root.view_end)
    } else {
        root.view_start
    };
    let range = view
        .audio_event_editor
        .range
        .filter(|_| same_owner)
        .and_then(|(start, end)| {
            let start = start.max(root.view_start);
            let end = end.min(root.view_end);
            (start < end).then_some((start, end))
        });
    let selected_range = if same_context {
        match view
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
            _ => range,
        }
    } else {
        range
    };
    // Allocate only scalar projection scratch outside the guard, then fill it
    // without growth from the same retained root before changing local state.
    let mut positions = Vec::with_capacity(preview.events.len() * 3 + 5);
    positions.extend([root.view_start, root.view_end, cursor]);
    if let Some((a, b)) = selected_range {
        positions.extend([a, b]);
    }
    for event in &preview.events {
        let a = event.start.max(root.view_start);
        let b = event.end.min(root.view_end);
        if a >= b {
            continue;
        }
        positions.extend([a, b]);
        if let Some(original) = events.events.iter().find(|e| e.id == event.id).or_else(|| {
            events.events.iter().find(|e| {
                e.source_start <= event.source_start
                    && event.source_start < e.source_start + e.length()
            })
        }) {
            positions.push(prepared_waveform_position(*event, *original, a)?);
        }
    }
    let mut frames = Vec::with_capacity(positions.len());
    let projection = view.event_read(|snapshot, read| {
        for position in &positions {
            if frames.len() == frames.capacity() {
                return Err(Invalid("audio event projection buffer is full"));
            }
            frames.push((*position, read.root_frame_at(*position)?));
        }
        let project = snapshot.model();
        let clip_start = project
            .clip(clip_id)
            .ok_or(Invalid("audio event clip is unavailable"))?
            .start();
        Ok::<_, Invalid>((
            clip_start,
            super::super::musical_time::grid_lines(&project, viewport, width, grid),
            super::super::musical_time::ruler_ticks(&project, viewport, width),
        ))
    });
    let (clip_start, lines, ticks) = match projection {
        Ok(Some(Ok(value))) => value,
        Ok(Some(Err(error))) => {
            view.event_error(error.into());
            return None;
        }
        Ok(None) => return None,
        Err(error) => {
            view.event_error(error);
            return None;
        }
    };
    let frame_at = |position| {
        frames
            .iter()
            .find(|(p, _)| *p == position)
            .map(|(_, frame)| *frame)
    };
    let first = frame_at(root.view_start)?;
    let frame_to_x =
        |frame: u64| frame.saturating_sub(first + local.start) as f32 / local.length.max(1) as f32;
    if view.audio_event_editor.owner != Some(owner) {
        let clipboard = view.audio_event_editor.clipboard.take();
        view.audio_event_editor = State {
            owner: Some(owner),
            revision,
            source_key: Some(source_key),
            cursor: root.view_start,
            clipboard,
            ..State::default()
        };
    }
    if view.audio_event_editor.revision != revision
        || view.audio_event_editor.source_key != Some(source_key)
    {
        view.audio_event_editor.gesture = None;
        view.audio_event_editor.cursor = cursor;
        view.audio_event_editor.range = range;
        view.audio_event_editor.revision = revision;
        view.audio_event_editor.source_key = Some(source_key);
    }
    if view.audio_event_editor.selected.is_some_and(|id| {
        !events
            .events
            .iter()
            .any(|e| e.id == id && e.start >= root.view_start && e.end <= root.view_end)
    }) {
        view.audio_event_editor.selected = None;
    }
    let mut body = div()
        .id("audio-events-waveform")
        .debug_selector(|| "audio-events-waveform".into())
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .cursor_crosshair();
    for line in lines {
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
    body = body.child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(22.0))
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border))),
    );
    for tick in ticks {
        let x = (tick.frame - viewport.start) as f32 / viewport.length.max(1) as f32;
        body = body.child(
            div()
                .absolute()
                .top_0()
                .left(relative(x))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text(tick.label)),
        );
    }
    for event in &preview.events {
        let raw_a = event.start.max(root.view_start);
        let raw_b = event.end.min(root.view_end);
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
            .cursor(gpui::CursorStyle::OpenHand);
        if let Some(original) = events.events.iter().find(|e| e.id == event.id).or_else(|| {
            events.events.iter().find(|e| {
                e.source_start <= event.source_start
                    && event.source_start < e.source_start + e.length()
            })
        }) {
            let source_raw = prepared_waveform_position(*event, *original, raw_a)?;
            let source_root = frame_at(source_raw)?;
            let source_frame = clip_start
                .checked_add(source_root.saturating_sub(first))?
                .checked_add(visible_start - start)?;
            let end_frame = source_frame.checked_add(visible_end - visible_start)?;
            // The shared renderer reads current accepted peaks through the
            // editor's retained capability during paint; no owner escapes here.
            let segment = waveform::event_segment_view(
                view,
                cx,
                waveform::EventSegmentRequest {
                    clip: clip_id,
                    source_root_start: source_root + (visible_start - start),
                    source_root_end: source_root + (visible_end - start),
                    source_project_start: source_frame,
                    source_project_end: end_frame,
                    generation: view.project_generation,
                    revision,
                    source_key,
                },
            );
            match segment {
                Ok(Some(segment)) => element = element.child(segment),
                Ok(None) => {}
                Err(error) => {
                    view.event_error(error.into());
                    return None;
                }
            }
        }
        element = element.child(
            div()
                .absolute()
                .top_0()
                .left(px(3.0))
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .child(theme::text(format!("event {}", event.id))),
        );
        if event.start >= root.view_start && start >= first + local.start {
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
        if event.end <= root.view_end && end <= first + local.start + local.length {
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
        body = body.child(element);
    }
    if let Some((a, b)) = selected_range.filter(|(a, b)| a < b) {
        let a = frame_at(a)?.max(first + local.start);
        let b = frame_at(b)?.min(first + local.start + local.length);
        if a < b {
            body = body.child(
                div()
                    .debug_selector(|| "audio-event-time-selection".into())
                    .absolute()
                    .top(px(22.0))
                    .bottom_0()
                    .left(relative(frame_to_x(a)))
                    .w(relative((b - a) as f32 / local.length.max(1) as f32))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(0.2),
            );
        }
    }
    let cursor = frame_at(cursor)?;
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
    let entity = cx.entity();
    body = body.child(
        canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                entity.update(cx, |view, cx| {
                    let changed = view.audio_event_editor.bounds != Some(bounds);
                    view.audio_event_editor.bounds = Some(bounds);
                    if changed {
                        view.audio_event_editor.gesture = None;
                        cx.notify();
                    }
                });
            },
        )
        .absolute()
        .inset_0(),
    );
    body = body
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                if view.edit_pending() || !matches!(view.event_context_current(), Ok(true)) {
                    return;
                }
                let Ok(Some(position)) = view
                    .event_pointer_position(f32::from(event.position.x), event.modifiers.platform)
                else {
                    return;
                };
                let Ok(Some(raw)) = view.event_pointer_position(
                    f32::from(event.position.x),
                    view.audio_event_editor.grid != SnapGrid::Off,
                ) else {
                    return;
                };
                let data = match view.event_data() {
                    Ok(Some(data)) => data,
                    Ok(None) => return,
                    Err(error) => {
                        view.event_error(error);
                        return;
                    }
                };
                let root = data.root;
                let events = data.events;
                let ruler = view
                    .audio_event_editor
                    .bounds
                    .is_some_and(|b| f32::from(event.position.y - b.origin.y) < 22.0);
                let hit = events
                    .events
                    .iter()
                    .find(|e| e.start <= raw && raw < e.end)
                    .copied();
                let range_hit = view
                    .audio_event_editor
                    .range
                    .filter(|(a, b)| a <= &raw && raw < *b);
                let kind = if !ruler && !event.modifiers.shift && range_hit.is_some() {
                    if let Some((start, end)) = range_hit {
                        Kind::RangeMove { start, end }
                    } else {
                        Kind::Select
                    }
                } else if ruler || event.modifiers.shift || hit.is_none() {
                    Kind::Select
                } else if let Some(hit) = hit {
                    let Ok(Some(first)) = view.event_frame_at(root.view_start) else {
                        return;
                    };
                    let Ok(Some(local)) = view.event_viewport(&root) else {
                        return;
                    };
                    let Ok(Some(hit_start)) = view.event_frame_at(hit.start) else {
                        return;
                    };
                    let Ok(Some(hit_end)) = view.event_frame_at(hit.end) else {
                        return;
                    };
                    let bounds = view.audio_event_editor.bounds;
                    let width = bounds.map_or(1.0, |b| f32::from(b.size.width));
                    let offset = bounds.map_or(0.0, |b| f32::from(event.position.x - b.origin.x));
                    let x = |frame: u64| {
                        frame.saturating_sub(first + local.start) as f32
                            / local.length.max(1) as f32
                            * width
                    };
                    if hit.start >= root.view_start
                        && hit_start >= first + local.start
                        && (offset - x(hit_start)).abs() <= 6.0
                    {
                        Kind::Leading(hit)
                    } else if hit.end <= root.view_end
                        && hit_end <= first + local.start + local.length
                        && (offset - x(hit_end)).abs() <= 6.0
                    {
                        Kind::Trailing(hit)
                    } else {
                        Kind::Move(hit)
                    }
                } else {
                    Kind::Select
                };
                view.audio_event_focus.focus(window);
                match kind {
                    Kind::Select => {
                        view.audio_event_editor.range = Some((position, position));
                        view.audio_event_editor.selected = None;
                    }
                    Kind::Move(hit) | Kind::Leading(hit) | Kind::Trailing(hit) => {
                        view.audio_event_editor.range = None;
                        view.audio_event_editor.selected = Some(hit.id);
                    }
                    Kind::RangeMove { .. } => {}
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
        )
        .on_mouse_move(cx.listener(|view, event: &gpui::MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left)
                && view.audio_event_editor.gesture.is_some()
            {
                view.event_update_pointer(f32::from(event.position.x), event.modifiers.platform);
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|view, event: &gpui::MouseUpEvent, _, cx| {
                if view.audio_event_editor.gesture.is_none() {
                    return;
                }
                if !view.event_update_pointer(f32::from(event.position.x), event.modifiers.platform)
                {
                    return;
                }
                if let Some(gesture) = view.audio_event_editor.gesture.as_ref().cloned() {
                    if let Some(operation) = gesture.operation {
                        let moved = match &operation {
                            AudioEventEdit::MoveRange {
                                start,
                                end,
                                destination,
                            } => Some((*destination, *destination + (*end - *start))),
                            _ => None,
                        };
                        if view.event_commit(operation)
                            && let Some((start, end)) = moved
                        {
                            view.audio_event_editor.range = Some((start, end));
                            view.audio_event_editor.cursor = start;
                        }
                    } else {
                        view.audio_event_editor.gesture = None;
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|view, _, window, cx| {
                if view.audio_event_editor.gesture.take().is_some() {
                    view.audio_event_focus.focus(window);
                    cx.notify();
                }
            }),
        );

    let grid = view.audio_event_editor.grid;
    let toolbar = div()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .child(
            waveform::button("✂", "split audio event · cmd+e", ("audio-event-split", 0)).on_click(
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
        .child(
            waveform::button("↔", "fit audio events · cmd+0", ("audio-event-fit", 0)).on_click(
                cx.listener(|view, _, window, cx| {
                    view.audio_event_focus.focus(window);
                    view.audio_event_editor.viewport = None;
                    cx.notify();
                }),
            ),
        )
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
        .child(body)
        .on_action(
            cx.listener(|view, _: &super::super::ToggleSnapGrid, _, cx| {
                view.audio_event_editor.grid = if view.audio_event_editor.grid == SnapGrid::Off {
                    SnapGrid::Adaptive
                } else {
                    SnapGrid::Off
                };
                view.audio_event_editor.gesture = None;
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.listener(|view, _: &super::super::MoveLeft, window, cx| {
            view.event_nudge(false, false, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.listener(|view, _: &super::super::MoveRight, window, cx| {
                view.event_nudge(true, false, window.modifiers().platform);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.listener(|view, _: &super::super::TrimLeft, window, cx| {
            view.event_nudge(false, true, window.modifiers().platform);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.listener(|view, _: &super::super::TrimRight, window, cx| {
                view.event_nudge(true, true, window.modifiers().platform);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.listener(|view, _: &super::super::DuplicateClip, _, cx| {
            view.event_duplicate();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.listener(|view, _: &super::super::SelectAllMidiNotes, _, cx| {
                view.event_select_all();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::InsertTimeSelection, _, cx| {
                view.event_paste(true);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::CopyTimeSelection, _, cx| {
                view.event_copy(false);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::CutTimeSelection, _, cx| {
                view.event_copy(true);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::PasteTimeSelection, _, cx| {
                view.event_paste(false);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(cx.listener(|view, _: &super::super::DeleteClip, _, cx| {
            view.event_delete();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::super::SplitClip, _, cx| {
            view.event_split();
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::super::ZoomIn, _, cx| {
            view.event_zoom(true);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::super::ZoomOut, _, cx| {
            view.event_zoom(false);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &super::super::FitTimeline, _, cx| {
            view.audio_event_editor.viewport = None;
            cx.stop_propagation();
            cx.notify();
        }))
        .on_action(
            cx.listener(|view, _: &super::super::PanTimelineLeft, _, cx| {
                view.event_pan(false);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::PanTimelineRight, _, cx| {
                view.event_pan(true);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_action(
            cx.listener(|view, _: &super::super::CancelDragOrStop, _, cx| {
                view.audio_event_editor.gesture = None;
                view.audio_event_editor.range = None;
                cx.stop_propagation();
                cx.notify();
            }),
        );
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
