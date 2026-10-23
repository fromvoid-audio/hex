//! Clip-local loop-brace view. Shares the source ruler's single immutable
//! capture/preview/edit owner; no transport, source-growth or Arrangement loop API.
//! Disabled loops retain editable bounds. Edge drags preserve the opposite edge;
//! body drags preserve length, constrained by the existing source and playback start.
use super::*;

fn x_at(start: u64, span: u64, bounds: CanvasBounds, beat: u64) -> Option<f32> {
    let offset = beat.checked_sub(start)?;
    (span > 0 && offset <= span)
        .then(|| bounds.x + (offset as f64 / span as f64) as f32 * bounds.width)
}
fn interval(
    start: u64,
    span: u64,
    bounds: CanvasBounds,
    values: SourceValues,
) -> Option<(f32, f32)> {
    let end = start.checked_add(span)?;
    let first = values.loop_start.max(start);
    let last = values.loop_end.min(end);
    if first >= last {
        return None;
    }
    Some((
        x_at(start, span, bounds, first)?,
        x_at(start, span, bounds, last)?,
    ))
}
fn hit(context: Context, x: f32) -> Option<Handle> {
    if !x.is_finite() {
        return None;
    }
    let first = context
        .x(context.values.loop_start)
        .filter(|at| (x - at).abs() <= 6.0);
    let last = context
        .x(context.values.loop_end)
        .filter(|at| (x - at).abs() <= 6.0);
    match (first, last) {
        (Some(a), Some(b)) => Some(if x <= a / 2.0 + b / 2.0 {
            Handle::LoopStart
        } else {
            Handle::LoopEnd
        }),
        (Some(_), None) => Some(Handle::LoopStart),
        (None, Some(_)) => Some(Handle::LoopEnd),
        _ => interval(context.start, context.span, context.bounds, context.values)
            .filter(|(first, last)| x >= *first && x <= *last)
            .map(|_| Handle::LoopBody),
    }
}
fn beat_text(raw: u64) -> String {
    let beats = raw as f64 / hex_project::BeatTime::ONE.raw() as f64;
    // One-tick loops are valid; never display a positive duration as zero.
    if raw > 0 && beats < 0.0001 {
        return format!("{beats:.2e}");
    }
    let mut text = format!("{beats:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}
impl EditorView {
    pub(super) fn midi_loop_ruler(&mut self, cx: &mut ViewContext<Self>) -> Div {
        let placeholder = CanvasBounds {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let rendered = self.midi_ruler_context(placeholder);
        let values = self
            .midi_ruler
            .gesture
            .map(|gesture| gesture.preview)
            .or_else(|| {
                self.midi_clip().map(|source| {
                    SourceValues::from_source(source, 0, source.source_length().raw())
                })
            });
        let Some(values) = values else {
            return div();
        };
        let start = self.piano_beat_start.raw();
        let span = self.piano_beat_span.raw();
        let active = self
            .midi_ruler
            .gesture
            .filter(|gesture| gesture.handle != Handle::Start);
        let label = format!(
            "{}–{}b · {}b",
            beat_text(values.loop_start),
            beat_text(values.loop_end),
            beat_text(values.loop_end - values.loop_start)
        );
        let description = format!(
            "clip loop {} · {} · drag either end or move brace · hold cmd/control to invert snapping",
            if values.looping {
                "on"
            } else {
                "off; markers retained"
            },
            label
        );
        let color = theme::color(if values.looping {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Muted
        });
        let entity = cx.entity();
        let brace = div()
            .id("midi-loop-ruler")
            .debug_selector(|| "midi-loop-ruler".into())
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .track_focus(&self.midi_focus)
            .key_context("hex_midi")
            .child(
                div()
                    .absolute()
                    .right(px(3.0))
                    .top_0()
                    .max_w(px(240.0))
                    .truncate()
                    .text_size(px(8.0))
                    .text_color(rgb(color))
                    .child(theme::text(label)),
            )
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        let b = bounds_of(bounds);
                        if !b.is_valid() || span == 0 {
                            return;
                        }
                        if let Some((first, last)) = interval(start, span, b, values) {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(first), bounds.origin.y + px(10.0)),
                                    size(px(last - first), px(3.0)),
                                ),
                                rgb(color),
                            ));
                        }
                        for marker in [values.loop_start, values.loop_end] {
                            if let Some(x) = x_at(start, span, b, marker) {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(px(x - 1.5), bounds.origin.y + px(4.0)),
                                        size(px(3.0), px(9.0)),
                                    ),
                                    rgb(color),
                                ));
                            }
                        }
                        let target = active.map(|gesture| gesture.handle).or_else(|| {
                            hitbox
                                .is_hovered(window)
                                .then(|| {
                                    rendered.and_then(|context| {
                                        hit(
                                            Context {
                                                bounds: b,
                                                ..context
                                            },
                                            f32::from(window.mouse_position().x),
                                        )
                                    })
                                })
                                .flatten()
                        });
                        if let Some(target) = target {
                            window.set_cursor_style(
                                match target {
                                    Handle::LoopBody => {
                                        if active.is_some() {
                                            gpui::CursorStyle::ClosedHand
                                        } else {
                                            gpui::CursorStyle::OpenHand
                                        }
                                    }
                                    Handle::LoopStart => gpui::CursorStyle::ResizeLeft,
                                    _ => gpui::CursorStyle::ResizeRight,
                                },
                                &hitbox,
                            );
                        }
                        let down_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if !phase.bubble()
                                || event.button != MouseButton::Left
                                || !bounds.contains(&event.position)
                            {
                                return;
                            }
                            down_entity.update(cx, |view, cx| {
                                if let Some(context) = rendered.map(|context| Context {
                                    bounds: b,
                                    ..context
                                }) && let Some(handle) =
                                    hit(context, f32::from(event.position.x))
                                    && view.begin_midi_ruler(
                                        context,
                                        f32::from(event.position.x),
                                        handle,
                                    )
                                {
                                    view.midi_focus.focus(window);
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            });
                        });
                        let move_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if !phase.bubble() {
                                return;
                            }
                            move_entity.update(cx, |view, cx| {
                                if view
                                    .midi_ruler
                                    .gesture
                                    .is_none_or(|gesture| gesture.handle == Handle::Start)
                                {
                                    return;
                                }
                                let changed = if event.pressed_button == Some(MouseButton::Left) {
                                    view.update_midi_ruler(
                                        b,
                                        f32::from(event.position.x),
                                        event.modifiers.platform,
                                    )
                                } else {
                                    view.midi_ruler.cancel()
                                };
                                if changed {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            });
                        });
                        let up_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if !phase.bubble() || event.button != MouseButton::Left {
                                return;
                            }
                            up_entity.update(cx, |view, cx| {
                                if view
                                    .midi_ruler
                                    .gesture
                                    .is_some_and(|gesture| gesture.handle != Handle::Start)
                                {
                                    view.finish_midi_ruler(
                                        b,
                                        f32::from(event.position.x),
                                        event.modifiers.platform,
                                    );
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            });
                        });
                        let wheel_entity = entity.clone();
                        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                            if !phase.capture() || !bounds.contains(&event.position) {
                                return;
                            }
                            let delta = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                            wheel_entity.update(cx, |view, cx| {
                                if view.zoom_piano_wheel_at(
                                    [b.x, b.y, b.width, b.height],
                                    f32::from(event.position.x),
                                    delta,
                                ) {
                                    cx.notify();
                                }
                            });
                            cx.stop_propagation();
                        });
                    },
                )
                .size_full(),
            );
        div()
            .h(px(16.0))
            .flex_shrink_0()
            .flex()
            .child(
                div()
                    .id("midi-loop-ruler-toggle")
                    .debug_selector(|| "midi-loop-ruler-toggle".into())
                    .w(px(38.0))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(color))
                    .child(theme::text(if values.looping { "↻" } else { "○" }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::super::transport::MiniTooltip {
                            description: description.clone().into(),
                        })
                        .into()
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.cancel_midi_note_gesture();
                        if let Some(context) = rendered {
                            view.edit_midi_source(
                                context.clip,
                                context.revision,
                                context.generation,
                                midi_source_controls::SourceField::Looping,
                                1,
                            );
                            view.midi_focus.focus(window);
                            cx.notify();
                        }
                    })),
            )
            .children([brace])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_feedback_distinguishes_zero_and_the_smallest_positive_loop() {
        assert_eq!(beat_text(0), "0");
        assert!(beat_text(1).parse::<f64>().unwrap() > 0.0);
        assert_eq!(beat_text(hex_project::BeatTime::ONE.raw()), "1");
        assert_eq!(beat_text(hex_project::BeatTime::ONE.raw() / 4), "0.25");
    }
    #[test]
    fn loop_hits_keep_clipped_edges_honest_and_tiny_endpoints_accessible() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = super::super::geometry_tests::context();
        let c = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: 3 * beat,
                ..base.values
            },
            ..base
        };
        assert_eq!(hit(c, 200.0), Some(Handle::LoopStart));
        assert_eq!(hit(c, 400.0), Some(Handle::LoopEnd));
        assert_eq!(hit(c, 300.0), Some(Handle::LoopBody));
        assert_eq!(hit(c, 100.0), None);
        let clipped = Context {
            start: 2 * beat,
            span: 2 * beat,
            ..c
        };
        assert_eq!(hit(clipped, 101.0), Some(Handle::LoopBody));
        let tiny = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: beat + 1,
                ..base.values
            },
            ..base
        };
        assert_eq!(hit(tiny, 198.0), Some(Handle::LoopStart));
        assert_eq!(hit(tiny, 202.0), Some(Handle::LoopEnd));
        assert!(hit(c, f32::NAN).is_none());
    }
}
