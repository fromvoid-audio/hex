use super::super::waveform;
use super::*;
use crate::theme;
use gpui::{Context, Div, MouseButton, canvas, div, prelude::*, px, relative, rgb};

pub(in crate::editor) fn view(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Option<gpui::Stateful<Div>> {
    let (clip_id, root, revision) = view.event_context()?;
    let owner = (clip_id, view.project_generation);
    if view.audio_event_editor.owner != Some(owner) {
        let clipboard = view.audio_event_editor.clipboard.take();
        view.audio_event_editor = State {
            owner: Some(owner),
            revision,
            cursor: root.view_start,
            clipboard,
            ..State::default()
        };
    }
    if view.audio_event_editor.revision != revision {
        view.audio_event_editor.gesture = None;
        view.audio_event_editor.revision = revision;
    }
    let state = view.state.as_ref()?;
    let clip = state.project.clip(clip_id)?.clone();
    let events = clip.audio_events();
    if view
        .audio_event_editor
        .selected
        .is_some_and(|id| !events.events.iter().any(|e| e.id == id))
    {
        view.audio_event_editor.selected = None;
    }
    let preview = view
        .audio_event_editor
        .gesture
        .as_ref()
        .and_then(|g| g.operation.clone())
        .and_then(|op| events.edited(op, root.domain.clock.length()).ok())
        .unwrap_or_else(|| events.clone());
    let local = view.event_viewport(&root);
    let viewport = view.event_project_viewport(&root)?;
    let first = root
        .root_frame_at(root.view_start, state.project.tempo_map())
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
        .debug_selector(|| "audio-events-waveform".into())
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .cursor_crosshair();
    for line in super::super::musical_time::grid_lines(
        &state.project,
        viewport,
        width,
        view.audio_event_editor.grid,
    ) {
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
    for tick in super::super::musical_time::ruler_ticks(&state.project, viewport, width) {
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
            let source_raw = original
                .start
                .checked_add(event.source_start.saturating_sub(original.source_start))?
                .checked_add(raw_a - event.start)?;
            let source_root = frame_at(source_raw)?;
            let source_frame = clip
                .start()
                .checked_add(source_root.saturating_sub(first))?
                .checked_add(visible_start - start)?;
            let end_frame = source_frame.checked_add(visible_end - visible_start)?;
            if let Some(prepared) = state.audio_clip_waveforms.get(&clip_id) {
                element = element.child(waveform::waveform_source_segment_gain_view_cached(
                    std::sync::Arc::clone(&prepared.waveform),
                    0,
                    source_root + (visible_start - start),
                    source_root + (visible_end - start),
                    clip.gain(),
                    std::rc::Rc::clone(&view.waveform_geometry_cache),
                ));
            } else if let Some(wave) = state.waveforms.get(&clip.asset()) {
                element = element.child(waveform::waveform_view_cached(
                    wave,
                    &clip,
                    source_frame,
                    end_frame,
                    std::rc::Rc::clone(&view.waveform_geometry_cache),
                ));
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
                    .top(px(22.0))
                    .bottom_0()
                    .left(relative(frame_to_x(a)))
                    .w(relative((b - a) as f32 / local.length.max(1) as f32))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(0.2),
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
                if view.edit_pending() || !view.event_context_current() {
                    return;
                }
                let Some(position) = view
                    .event_pointer_position(f32::from(event.position.x), event.modifiers.platform)
                else {
                    return;
                };
                let Some(raw) = view.event_pointer_position(
                    f32::from(event.position.x),
                    view.audio_event_editor.grid != SnapGrid::Off,
                ) else {
                    return;
                };
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
                let ruler = view
                    .audio_event_editor
                    .bounds
                    .is_some_and(|b| f32::from(event.position.y - b.origin.y) < 22.0);
                let hit = clip
                    .audio_events()
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
                    view.audio_event_editor.range = Some((position, position));
                    view.audio_event_editor.selected = None;
                    Kind::Select
                } else if let Some(hit) = hit {
                    view.audio_event_editor.range = None;
                    view.audio_event_editor.selected = Some(hit.id);
                    let first = root
                        .root_frame_at(root.view_start, state.project.tempo_map())
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
                    if hit.start >= root.view_start
                        && root
                            .root_frame_at(hit.start, state.project.tempo_map())
                            .is_ok_and(|frame| frame >= first + local.start)
                        && (offset - x(hit.start)).abs() <= 6.0
                    {
                        Kind::Leading(hit)
                    } else if hit.end <= root.view_end
                        && root
                            .root_frame_at(hit.end, state.project.tempo_map())
                            .is_ok_and(|frame| frame <= first + local.start + local.length)
                        && (offset - x(hit.end)).abs() <= 6.0
                    {
                        Kind::Trailing(hit)
                    } else {
                        Kind::Move(hit)
                    }
                } else {
                    Kind::Select
                };
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
                view.event_update_pointer(f32::from(event.position.x), event.modifiers.platform);
                if let Some(gesture) = view.audio_event_editor.gesture.take() {
                    if let Some(operation) = gesture.operation {
                        if let AudioEventEdit::MoveRange {
                            start,
                            end,
                            destination,
                        } = &operation
                        {
                            view.audio_event_editor.range =
                                Some((*destination, *destination + (*end - *start)));
                            view.audio_event_editor.cursor = *destination;
                        }
                        view.event_commit(operation);
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|view, _, window, cx| {
                view.audio_event_focus.focus(window);
                if view.audio_event_editor.gesture.take().is_some() {
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
                view.audio_event_editor.grid = view.audio_event_editor.grid.next();
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
