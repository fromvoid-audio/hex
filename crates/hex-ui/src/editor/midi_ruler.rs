//! Source-local piano ruler and playback-start marker. This owns only a captured
//! preview; release uses the existing source edit command and Session ACK owner.
//! Source start is neither the insertion cursor nor the Arrangement playhead.
use super::{
    EditorView,
    midi_drag::{self, MidiGrid},
    midi_source_controls::{self, SourceValues},
    midi_stroke::CanvasBounds,
};
use crate::theme;
use gpui::{
    Bounds, Context as ViewContext, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, ScrollWheelEvent, canvas, div, fill, point, prelude::*, px, relative, rgb, size,
};
use hex_project::ClipId;
use hex_session::service::Request;

#[derive(Default)]
pub(super) struct State {
    gesture: Option<Gesture>,
}
impl State {
    pub(super) fn cancel(&mut self) -> bool {
        self.gesture.take().is_some()
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Context {
    generation: u64,
    revision: u64,
    clip: ClipId,
    values: SourceValues,
    start: u64,
    span: u64,
    grid: MidiGrid,
    bounds: CanvasBounds,
}
impl Context {
    fn exclusive_end(self) -> u64 {
        if self.values.looping {
            self.values.loop_end
        } else {
            self.values.source_length
        }
    }
    fn valid(self) -> bool {
        self.bounds.is_valid()
            && self.span > 0
            && self.start.checked_add(self.span).is_some()
            && self.values.source_length > 0
            && self.values.loop_start < self.values.loop_end
            && self.values.loop_end <= self.values.source_length
            && self.values.source_start < self.exclusive_end()
    }
    fn x(self, beat: u64) -> Option<f32> {
        let offset = beat.checked_sub(self.start)?;
        (offset <= self.span)
            .then(|| self.bounds.x + (offset as f64 / self.span as f64) as f32 * self.bounds.width)
    }
}
#[derive(Clone, Copy)]
struct Gesture {
    context: Context,
    down_x: f32,
    preview: u64,
}
impl Gesture {
    fn update(&mut self, x: f32, invert_snap: bool) -> bool {
        if !x.is_finite() {
            return false;
        }
        let dx = f64::from(x) - f64::from(self.down_x);
        // A stationary click on an off-grid marker must remain a true no-op.
        let next = if dx.abs() < 3.0 {
            self.context.values.source_start
        } else {
            let ticks = (dx / f64::from(self.context.bounds.width) * self.context.span as f64)
                .round()
                .clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
            let raw = (i128::from(self.context.values.source_start) + ticks)
                .clamp(0, i128::from(u64::MAX)) as u64;
            let grid = midi_drag::effective_grid(self.context.grid, invert_snap);
            let last = self.context.exclusive_end() - 1;
            let last = grid.step_raw().map_or(last, |step| last / step * step);
            midi_drag::snap_raw(raw, grid).min(last)
        };
        let changed = next != self.preview;
        self.preview = next;
        changed
    }
}
fn bounds_of(bounds: Bounds<gpui::Pixels>) -> CanvasBounds {
    CanvasBounds {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    }
}
impl EditorView {
    fn midi_ruler_context(&self, bounds: CanvasBounds) -> Option<Context> {
        if self.edit_pending() || self.context_menu.is_some() {
            return None;
        }
        let state = self.state.as_ref()?;
        let clip = self.detail_clip_id()?;
        let source = state.project.midi_source(clip)?;
        let arrangement = state.project.midi_clip(clip);
        let context = Context {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            values: SourceValues::from_source(
                source,
                arrangement.map_or(0, |clip| clip.start().raw()),
                arrangement.map_or(source.source_length().raw(), |clip| {
                    clip.arrangement_length().raw()
                }),
            ),
            start: self.piano_beat_start.raw(),
            span: self.piano_beat_span.raw(),
            grid: self.midi_snap_grid,
            bounds,
        };
        context.valid().then_some(context)
    }
    pub(super) fn reconcile_midi_ruler(&mut self) {
        if self.midi_ruler.gesture.is_some_and(|gesture| {
            self.midi_ruler_context(gesture.context.bounds) != Some(gesture.context)
        }) {
            self.midi_ruler.cancel();
        }
    }
    fn begin_midi_start(&mut self, context: Context, x: f32) -> bool {
        if !x.is_finite()
            || self.midi_ruler_context(context.bounds) != Some(context)
            || context
                .x(context.values.source_start)
                .is_none_or(|marker| (marker - x).abs() > 7.0)
        {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_hover = None;
        self.midi_ruler.gesture = Some(Gesture {
            context,
            down_x: x,
            preview: context.values.source_start,
        });
        true
    }
    fn update_midi_start(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) -> bool {
        let Some(gesture) = self.midi_ruler.gesture else {
            return false;
        };
        if !x.is_finite() || self.midi_ruler_context(bounds) != Some(gesture.context) {
            return self.midi_ruler.cancel();
        }
        self.midi_ruler
            .gesture
            .as_mut()
            .is_some_and(|gesture| gesture.update(x, invert_snap))
    }
    fn finish_midi_start(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) {
        let Some(mut gesture) = self.midi_ruler.gesture.take() else {
            return;
        };
        if !x.is_finite() || self.midi_ruler_context(bounds) != Some(gesture.context) {
            return;
        }
        gesture.update(x, invert_snap);
        if gesture.preview == gesture.context.values.source_start {
            return;
        }
        let changed = SourceValues {
            source_start: gesture.preview,
            ..gesture.context.values
        };
        if let Some(command) = self.state.as_ref().and_then(|state| {
            midi_source_controls::edit_command(&state.project, gesture.context.clip, changed)
        }) {
            self.enqueue(Request::Edit(command));
        }
    }
    pub(super) fn midi_source_ruler(&mut self, cx: &mut ViewContext<Self>) -> Div {
        let start = self.piano_beat_start.raw();
        let span = self.piano_beat_span.raw().max(1);
        let marker = self
            .midi_ruler
            .gesture
            .map(|gesture| gesture.preview)
            .or_else(|| self.midi_clip().map(|source| source.source_start().raw()));
        let rendered = self.midi_ruler_context(CanvasBounds {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        let active = self.midi_ruler.gesture.is_some();
        let entity = cx.entity();
        let mut ruler = div()
            .id("midi-source-ruler")
            .debug_selector(|| "midi-source-ruler".into())
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .track_focus(&self.midi_focus)
            .key_context("hex_midi");
        let mut step = self
            .midi_snap_grid
            .step_raw()
            .unwrap_or(hex_project::BeatTime::ONE.raw())
            .max(1);
        while span / step > 8 {
            step = step.saturating_mul(2);
        }
        let first = start.div_ceil(step);
        for index in 0..=8_u64 {
            let Some(raw) = first
                .checked_add(index)
                .and_then(|value| value.checked_mul(step))
            else {
                break;
            };
            let Some(offset) = raw.checked_sub(start).filter(|offset| *offset <= span) else {
                break;
            };
            let fraction = (offset as f64 / span as f64) as f32;
            ruler = ruler.child(
                div()
                    .absolute()
                    .left(relative(fraction))
                    .top_0()
                    .pl(px(3.0))
                    .max_w(px(64.0))
                    .truncate()
                    .text_size(px(8.0))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(format!(
                        "{:.2}b",
                        raw as f64 / hex_project::BeatTime::ONE.raw() as f64
                    ))),
            );
        }
        ruler = ruler.child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                move |bounds, hitbox, window, _| {
                    let b = bounds_of(bounds);
                    if !b.is_valid() {
                        return;
                    }
                    if let Some(marker) = marker
                        .and_then(|marker| marker.checked_sub(start))
                        .filter(|offset| *offset <= span)
                    {
                        let x =
                            bounds.origin.x + px((marker as f64 / span as f64) as f32 * b.width);
                        let y = bounds.origin.y + px(10.0);
                        let mut path = PathBuilder::fill();
                        path.move_to(point(x - px(5.0), y));
                        path.line_to(point(x + px(5.0), y));
                        path.line_to(point(x, y + px(6.0)));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(theme::color(theme::PaletteRole::Primary)));
                        }
                        window.paint_quad(fill(
                            Bounds::new(point(x, bounds.origin.y), size(px(1.0), px(b.height))),
                            rgb(theme::color(theme::PaletteRole::Primary)),
                        ));
                        if active
                            || (hitbox.is_hovered(window)
                                && (f32::from(window.mouse_position().x - x)).abs() <= 7.0)
                        {
                            window.set_cursor_style(gpui::CursorStyle::ResizeLeftRight, &hitbox);
                        }
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
                            }) && view.begin_midi_start(context, f32::from(event.position.x))
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
                            if view.midi_ruler.gesture.is_none() {
                                return;
                            }
                            let changed = if event.pressed_button == Some(MouseButton::Left) {
                                view.update_midi_start(
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
                            if view.midi_ruler.gesture.is_some() {
                                view.finish_midi_start(
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
        div().h(px(18.0)).flex_shrink_0().flex()
            .child(div().id("midi-source-ruler-label").w(px(38.0)).flex_shrink_0().text_size(px(8.0)).child(theme::text("start"))
                .tooltip(|_,cx|cx.new(|_|super::transport::MiniTooltip { description:"drag clip playback start · grid snapping · hold cmd/control to invert · no seek".into() }).into()))
            .child(ruler)
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_ruler_ui_tests.rs"]
mod tests;

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    fn context() -> Context {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "marker".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        Context {
            generation: 0,
            revision: 0,
            clip,
            values: SourceValues::from_source(
                editor.project().midi_source(clip).unwrap(),
                0,
                4 * BeatTime::ONE.raw(),
            ),
            start: 0,
            span: 4 * BeatTime::ONE.raw(),
            grid: MidiGrid::Sixteenth,
            bounds: CanvasBounds {
                x: 100.0,
                y: 20.0,
                width: 400.0,
                height: 16.0,
            },
        }
    }
    #[test]
    fn marker_snaps_to_half_open_bounds_with_intro_before_or_start_after_loop_start() {
        let beat = BeatTime::ONE.raw();
        let base = context();
        for looping in [false, true] {
            let c = Context {
                values: SourceValues {
                    loop_start: beat,
                    loop_end: 2 * beat,
                    looping,
                    ..base.values
                },
                ..base
            };
            assert!(c.valid()); // Intro begins before loop_start.
            let mut g = Gesture {
                context: c,
                down_x: 100.0,
                preview: 0,
            };
            assert!(g.update(250.0, false));
            assert_eq!(g.preview, beat * 3 / 2); // Also after loop_start.
            assert!(g.update(f32::MAX, false));
            assert_eq!(g.preview, c.exclusive_end() - beat / 4);
            assert!(g.update(f32::MAX, true));
            assert_eq!(g.preview, c.exclusive_end() - 1);
            assert!(g.update(-f32::MAX, false));
            assert_eq!(g.preview, 0);
            assert!(!g.update(f32::NAN, false));
            assert_eq!(g.preview, 0);
        }
        let c = Context {
            values: SourceValues {
                source_start: beat / 3,
                ..base.values
            },
            ..base
        };
        let mut g = Gesture {
            context: c,
            down_x: 100.0,
            preview: c.values.source_start,
        };
        assert!(!g.update(100.0, false));
        assert_eq!(g.preview, beat / 3);
    }
    #[test]
    fn invalid_or_overflowing_ruler_contexts_are_refused_without_source_growth() {
        let c = context();
        for invalid in [
            Context { span: 0, ..c },
            Context {
                start: u64::MAX,
                span: 1,
                ..c
            },
            Context {
                bounds: CanvasBounds {
                    width: f32::NAN,
                    ..c.bounds
                },
                ..c
            },
            Context {
                values: SourceValues {
                    source_start: c.values.source_length,
                    ..c.values
                },
                ..c
            },
            Context {
                values: SourceValues {
                    looping: true,
                    source_start: c.values.loop_end,
                    ..c.values
                },
                ..c
            },
            Context {
                values: SourceValues {
                    loop_start: c.values.loop_end,
                    ..c.values
                },
                ..c
            },
        ] {
            assert!(!invalid.valid());
        }
        let mut g = Gesture {
            context: c,
            down_x: 100.0,
            preview: 0,
        };
        g.update(f32::MAX, false);
        assert_eq!(g.context.values, c.values);
        assert!(g.preview < c.values.source_length);
    }
}
