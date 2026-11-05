//! Source-local piano ruler, playback start and retained loop brace. Owns one
//! captured preview; the Window lease owns held input only. Release uses the
//! existing source edit command and Session ACK owner.
//! Source start is neither the insertion cursor nor the playback cursor. Blank
//! clicks delegate to the cue owner; drags retain source-time selection.
use super::{
    EditorView,
    midi_drag::{self, MidiGrid},
    midi_source_controls::{self, SourceValues},
    midi_stroke::CanvasBounds,
};
use crate::theme;
use gpui::{
    Bounds, Context as ViewContext, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, ScrollWheelEvent, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use hex_project::ClipId;
use hex_session::service::Request;

#[path = "midi_loop.rs"]
mod loop_brace;
#[path = "midi_time_selection.rs"]
mod time_selection;
pub(super) use time_selection::State as TimeSelection;

#[derive(Default)]
pub(super) struct State {
    gesture: Option<Gesture>,
    marker_hovered: bool,
}
impl State {
    pub(super) fn held(&self) -> bool {
        self.gesture.is_some()
    }
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
    // A forward one-shot Arrangement region begins at its start marker, so
    // the marker stays inside song time and before the region end.
    play_start_region: bool,
    start: i128,
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
            && self.start == super::midi_scene::clamp_start(self.start, self.span)
            && self.start.checked_add(i128::from(self.span)).is_some()
            && self.values.source_length > 0
            && self.values.loop_start < self.values.loop_end
            && self.values.loop_end <= self.values.source_length
            && self.values.source_start < self.exclusive_end()
    }
    fn x(self, beat: u64) -> Option<f32> {
        let offset = i128::from(beat).checked_sub(self.start)?;
        (offset >= 0 && offset <= i128::from(self.span))
            .then(|| self.bounds.x + (offset as f64 / self.span as f64) as f32 * self.bounds.width)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Start,
    LoopStart,
    LoopEnd,
    LoopBody,
}
impl Handle {
    fn held_cursor(self) -> gpui::CursorStyle {
        match self {
            Self::Start => gpui::CursorStyle::ResizeLeftRight,
            Self::LoopStart => gpui::CursorStyle::ResizeLeft,
            Self::LoopEnd => gpui::CursorStyle::ResizeRight,
            Self::LoopBody => gpui::CursorStyle::ClosedHand,
        }
    }
}
#[derive(Clone, Copy)]
struct Gesture {
    context: Context,
    down_x: f32,
    handle: Handle,
    preview: SourceValues,
}
// Bounds are inclusive and ordered by the validated source context. Restrict
// snapping to legal grid points, rather than snapping then landing off-grid at a
// raw boundary. A sub-grid interval with no grid point keeps the legal raw clamp.
fn clamp_loop_grid(raw: u64, first: u64, last: u64, grid: MidiGrid) -> u64 {
    if let Some(step) = grid.step_raw() {
        let wide_step = u128::from(step);
        let snapped_first = u128::from(first).div_ceil(wide_step) * wide_step;
        let snapped_last = last / step * step;
        if snapped_first <= u128::from(snapped_last) {
            return raw.clamp(snapped_first as u64, snapped_last);
        }
    }
    raw.clamp(first, last)
}
impl Gesture {
    fn update(&mut self, x: f32, invert_snap: bool) -> bool {
        if !x.is_finite() {
            return false;
        }
        let dx = f64::from(x) - f64::from(self.down_x);
        let original = self.context.values;
        let mut next = original;
        // A stationary click on an off-grid marker remains a true no-op.
        if dx.abs() >= 3.0 {
            let ticks = (dx / f64::from(self.context.bounds.width) * self.context.span as f64)
                .round()
                .clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
            let grid = midi_drag::effective_grid(self.context.grid, invert_snap);
            let moved = |start: u64| {
                let raw = (i128::from(start) + ticks).clamp(0, i128::from(u64::MAX)) as u64;
                midi_drag::snap_raw(raw, grid)
            };
            match self.handle {
                Handle::Start => {
                    let mut first = 0;
                    let mut last = self.context.exclusive_end() - 1;
                    if self.context.play_start_region {
                        first = original
                            .source_start
                            .saturating_sub(original.arrangement_start);
                        last = last.min(
                            original
                                .source_start
                                .saturating_add(original.arrangement_length)
                                .saturating_sub(1),
                        );
                    }
                    let last = grid.step_raw().map_or(last, |step| last / step * step);
                    next.source_start = moved(original.source_start).min(last).max(first);
                }
                Handle::LoopStart => {
                    next.loop_start =
                        clamp_loop_grid(moved(original.loop_start), 0, original.loop_end - 1, grid)
                }
                Handle::LoopEnd => {
                    let first = if original.looping {
                        original.loop_start.max(original.source_start) + 1
                    } else {
                        original.loop_start + 1
                    };
                    next.loop_end = clamp_loop_grid(
                        moved(original.loop_end),
                        first,
                        original.source_length,
                        grid,
                    );
                }
                Handle::LoopBody => {
                    let length = original.loop_end - original.loop_start;
                    let first = if original.looping {
                        original.source_start.saturating_sub(length - 1)
                    } else {
                        0
                    };
                    next.loop_start = clamp_loop_grid(
                        moved(original.loop_start),
                        first,
                        original.source_length - length,
                        grid,
                    );
                    next.loop_end = next.loop_start + length;
                }
            }
        }
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
            play_start_region: arrangement
                .is_some_and(|clip| !clip.looping() && clip.content_view().is_none()),
            start: self.piano_beat_start,
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
    fn begin_midi_start(
        &mut self,
        context: Context,
        x: f32,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        if self.time_selection_key_held
            || !x.is_finite()
            || self.midi_ruler_context(context.bounds) != Some(context)
            || context
                .x(context.values.source_start)
                .is_none_or(|marker| (marker - x).abs() > 7.0)
        {
            return false;
        }
        // A marker hit must not fall through to time selection if admission is refused.
        let _ = self.begin_midi_ruler(context, x, Handle::Start, window, cx);
        true
    }
    fn begin_midi_ruler(
        &mut self,
        context: Context,
        x: f32,
        handle: Handle,
        window: &mut gpui::Window,
        cx: &mut ViewContext<Self>,
    ) -> bool {
        if !x.is_finite() || self.midi_ruler_context(context.bounds) != Some(context) {
            return false;
        }
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            handle.held_cursor(),
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.owns_midi_ruler_capture(context, handle) {
                        return;
                    }
                    let changed = if !view.clip_view_open
                        || view.device_panel_active
                        || !view.midi_focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        view.midi_ruler.cancel()
                    } else {
                        let invert_snap = moved
                            .map(|e| e.modifiers.platform)
                            .or_else(|| released.map(|e| e.modifiers.platform))
                            .unwrap_or(false);
                        if released.is_some() {
                            view.finish_midi_ruler(
                                context.bounds,
                                f32::from(position.x),
                                invert_snap,
                            );
                            true
                        } else {
                            view.update_midi_ruler(
                                context.bounds,
                                f32::from(position.x),
                                invert_snap,
                            )
                        }
                    };
                    if changed {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.owns_midi_ruler_capture(context, handle) {
                        view.midi_ruler.cancel();
                        cx.notify();
                    }
                });
            },
            cx,
        ) {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_hover = None;
        self.midi_ruler.gesture = Some(Gesture {
            context,
            down_x: x,
            handle,
            preview: context.values,
        });
        true
    }
    fn owns_midi_ruler_capture(&self, context: Context, handle: Handle) -> bool {
        self.midi_ruler
            .gesture
            .is_some_and(|g| g.context == context && g.handle == handle)
    }
    /// clip_detail_view reconciles the canonical context before either surface.
    /// Paint witnesses that held draft and its bounds without scanning the project.
    fn renew_midi_ruler_capture(
        &self,
        bounds: CanvasBounds,
        owner: gpui::EntityId,
        window: &mut gpui::Window,
    ) {
        if self.clip_view_open
            && !self.device_panel_active
            && self.midi_focus.is_focused(window)
            && let Some(gesture) = self.midi_ruler.gesture
            && gesture.context.bounds == bounds
            && gesture.context.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == gesture.context.revision)
        {
            window.renew_pointer_capture(owner, gesture.handle.held_cursor());
        }
    }
    fn update_midi_ruler(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) -> bool {
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
    fn finish_midi_ruler(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) {
        let Some(mut gesture) = self.midi_ruler.gesture.take() else {
            return;
        };
        if !x.is_finite() || self.midi_ruler_context(bounds) != Some(gesture.context) {
            return;
        }
        gesture.update(x, invert_snap);
        if gesture.preview == gesture.context.values {
            return;
        }
        let changed = gesture.preview;
        self.commit_midi_clip_loop(gesture.context.clip, changed);
    }
    pub(super) fn midi_source_ruler(&mut self, cx: &mut ViewContext<Self>) -> Div {
        let start = self.piano_beat_start;
        let span = self.piano_beat_span.raw().max(1);
        let marker = self
            .midi_ruler
            .gesture
            .map(|gesture| gesture.preview.source_start)
            .or_else(|| {
                self.detail_clip_id()
                    .and_then(|id| self.pending_clip_loop_midi(id))
                    .map(|v| v.source_start)
            })
            .or_else(|| self.midi_clip().map(|source| source.source_start().raw()));
        let rendered = self.midi_ruler_context(CanvasBounds {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        let active = self
            .midi_ruler
            .gesture
            .is_some_and(|gesture| gesture.handle == Handle::Start);
        let time_selection = self.midi_time_selection_bounds();
        let start_marker_enabled = !self.time_selection_key_held;
        let entity = cx.entity();
        let mut ruler = super::timeline_strip::ruler_surface()
            .id("midi-source-ruler")
            .debug_selector(|| "midi-source-ruler".into())
            .h_full()
            .flex_1()
            .min_w_0()
            .track_focus(&self.midi_focus)
            .key_context("hex_midi")
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::control("clip timeline"),
                    description: "click: cue playback · drag: select time · drag marker: set start"
                        .into(),
                })
                .into()
            });
        let mut step = self
            .midi_snap_grid
            .step_raw()
            .unwrap_or(hex_project::BeatTime::ONE.raw())
            .max(1);
        while span / step > 8 {
            step = step.saturating_mul(2);
        }
        let step = i128::from(step);
        let first = -(-start).div_euclid(step);
        for index in 0..=8_i128 {
            let Some(raw) = first
                .checked_add(index)
                .and_then(|value| value.checked_mul(step))
            else {
                break;
            };
            let Some(offset) = raw
                .checked_sub(start)
                .filter(|offset| *offset >= 0 && *offset <= i128::from(span))
            else {
                break;
            };
            let fraction = (offset as f64 / span as f64) as f32;
            ruler = ruler.children(super::timeline_strip::ruler_tick(
                fraction,
                format!(
                    "{:.2}b",
                    raw as f64 / hex_project::BeatTime::ONE.raw() as f64
                ),
                raw % i128::from(hex_project::BeatTime::ONE.raw() * 4) == 0,
                false,
                false,
            ));
        }
        ruler = ruler.child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                move |bounds, hitbox, window, cx| {
                    let b = bounds_of(bounds);
                    if !b.is_valid() {
                        return;
                    }
                    if let Some((first, last)) = time_selection {
                        let end = start.saturating_add(i128::from(span));
                        let first = i128::from(first).max(start);
                        let last = i128::from(last).min(end);
                        if first < last {
                            let left =
                                b.x + ((first - start) as f64 / span as f64) as f32 * b.width;
                            let right =
                                b.x + ((last - start) as f64 / span as f64) as f32 * b.width;
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(px(left), bounds.origin.y),
                                    size(px(right - left), px(b.height)),
                                ),
                                gpui::Hsla::from(rgb(theme::time_selection_fill()))
                                    .opacity(theme::TIME_SELECTION_OPACITY),
                            ));
                        }
                    }
                    if let Some(marker) = marker
                        .and_then(|marker| i128::from(marker).checked_sub(start))
                        .filter(|offset| *offset >= 0 && *offset <= i128::from(span))
                    {
                        let x =
                            bounds.origin.x + px((marker as f64 / span as f64) as f32 * b.width);
                        let hovered = start_marker_enabled
                            && hitbox.is_hovered(window)
                            && !window.is_pointer_dragging()
                            && (f32::from(window.mouse_position().x - x)).abs() <= 7.0;
                        let marker_color = rgb(theme::color(if active || hovered {
                            theme::PaletteRole::Text
                        } else {
                            theme::PaletteRole::Primary
                        }));
                        let y = bounds.origin.y + px(10.0);
                        let mut path = PathBuilder::fill();
                        path.move_to(point(x - px(5.0), y));
                        path.line_to(point(x + px(5.0), y));
                        path.line_to(point(x, y + px(6.0)));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, marker_color);
                        }
                        window.paint_quad(fill(
                            Bounds::new(point(x, bounds.origin.y), size(px(1.0), px(b.height))),
                            marker_color,
                        ));
                        if active || hovered {
                            window.set_cursor_style(gpui::CursorStyle::ResizeLeftRight, &hitbox);
                        } else if !start_marker_enabled {
                            window.set_cursor_style(gpui::CursorStyle::IBeam, &hitbox);
                        }
                    }
                    let hover_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.bubble() {
                            return;
                        }
                        let hovered = !window.is_pointer_dragging()
                            && bounds.contains(&event.position)
                            && marker
                                .and_then(|at| i128::from(at).checked_sub(start))
                                .filter(|at| *at >= 0 && *at <= i128::from(span))
                                .is_some_and(|at| {
                                    let x = bounds.left()
                                        + bounds.size.width * (at as f64 / span as f64) as f32;
                                    (f32::from(event.position.x - x)).abs() <= 7.0
                                });
                        hover_entity.update(cx, |view, cx| {
                            let hovered = hovered && !view.time_selection_key_held;
                            if view.midi_ruler.marker_hovered != hovered {
                                view.midi_ruler.marker_hovered = hovered;
                                cx.notify();
                            }
                        });
                    });
                    entity
                        .read(cx)
                        .renew_midi_ruler_capture(b, entity.entity_id(), window);
                    if let Some(context) = rendered {
                        entity.read(cx).renew_midi_time_selection_capture(
                            b,
                            context.clip,
                            entity.entity_id(),
                            window,
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
                            }) && (view.begin_midi_start(
                                context,
                                f32::from(event.position.x),
                                window,
                                cx,
                            ) || view.begin_midi_time_selection(
                                context,
                                f32::from(event.position.x),
                                event.modifiers.platform,
                                window,
                                cx,
                            )) {
                                view.midi_focus.focus(window);
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
        let source_row=div().h(px(super::timeline_strip::RULER_HEIGHT)).flex_shrink_0().flex()
            .child(div().id("midi-source-ruler-label").w(px(38.0)).flex_shrink_0().text_size(px(8.0)).child(theme::text("start"))
                .tooltip(|_,cx|cx.new(|_|super::transport::MiniTooltip { kind: crate::tooltips::Kind::control("clip start"), description:"drag start marker · drag blank ruler to select time · cmd+d duplicate · hold cmd/control to invert snap · no seek".into() }).into()))
            .children([ruler]);
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .children([self.midi_loop_ruler(cx), source_row])
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_ruler_ui_tests.rs"]
mod tests;
#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) use tests::roundtrip;

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    pub(super) fn context() -> Context {
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
            play_start_region: false,
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
                handle: Handle::Start,
                preview: c.values,
            };
            assert!(g.update(250.0, false));
            assert_eq!(g.preview.source_start, beat * 3 / 2); // Also after loop_start.
            assert!(g.update(f32::MAX, false));
            assert_eq!(g.preview.source_start, c.exclusive_end() - beat / 4);
            assert!(g.update(f32::MAX, true));
            assert_eq!(g.preview.source_start, c.exclusive_end() - 1);
            assert!(g.update(-f32::MAX, false));
            assert_eq!(g.preview.source_start, 0);
            assert!(!g.update(f32::NAN, false));
            assert_eq!(g.preview.source_start, 0);
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
            handle: Handle::Start,
            preview: c.values,
        };
        assert!(!g.update(100.0, false));
        assert_eq!(g.preview.source_start, beat / 3);
    }
    #[test]
    fn invalid_or_overflowing_ruler_contexts_are_refused_without_source_growth() {
        let c = context();
        for invalid in [
            Context { span: 0, ..c },
            Context {
                start: i128::from(u64::MAX),
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
            handle: Handle::Start,
            preview: c.values,
        };
        g.update(f32::MAX, false);
        assert_eq!(g.context.values, c.values);
        assert!(g.preview.source_start < c.values.source_length);
    }
    #[test]
    fn loop_edges_preserve_opposite_anchor_and_body_preserves_length_inside_source() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        let c = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: 3 * beat,
                ..base.values
            },
            ..base
        };
        for (handle, down, x, first, last) in [
            (Handle::LoopStart, 200.0, 300.0, 2 * beat, 3 * beat),
            (Handle::LoopEnd, 400.0, 300.0, beat, 2 * beat),
            (Handle::LoopBody, 300.0, 400.0, 2 * beat, 4 * beat),
            (Handle::LoopBody, 300.0, -f32::MAX, 0, 2 * beat),
        ] {
            let mut g = Gesture {
                context: c,
                down_x: down,
                handle,
                preview: c.values,
            };
            assert!(g.update(x, false));
            assert_eq!((g.preview.loop_start, g.preview.loop_end), (first, last));
            assert_eq!(
                g.preview,
                SourceValues {
                    loop_start: first,
                    loop_end: last,
                    ..c.values
                }
            );
            assert!(!g.update(x, false));
        }
    }

    #[test]
    fn loop_grid_limits_remain_snapped_or_use_a_legal_short_interval_fallback() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        let mut leading = Gesture {
            context: base,
            down_x: 100.0,
            handle: Handle::LoopStart,
            preview: base.values,
        };
        leading.update(f32::MAX, false);
        assert_eq!(leading.preview.loop_start, 4 * beat - beat / 4);
        leading.update(f32::MAX, true);
        assert_eq!(leading.preview.loop_start, 4 * beat - 1);
        let short = Context {
            values: SourceValues {
                source_start: beat + 2,
                source_length: beat + 4,
                loop_start: beat + 1,
                loop_end: beat + 3,
                looping: true,
                ..base.values
            },
            ..base
        };
        assert!(short.valid());
        for (handle, x, first, last) in [
            (Handle::LoopEnd, -f32::MAX, beat + 1, beat + 3),
            (Handle::LoopEnd, f32::MAX, beat + 1, beat + 4),
            (Handle::LoopBody, -f32::MAX, beat + 1, beat + 3),
            (Handle::LoopBody, f32::MAX, beat + 2, beat + 4),
        ] {
            let mut g = Gesture {
                context: short,
                down_x: 200.0,
                handle,
                preview: short.values,
            };
            g.update(x, false);
            assert_eq!((g.preview.loop_start, g.preview.loop_end), (first, last));
            assert!(g.preview.source_start < g.preview.loop_end);
            assert!(g.preview.loop_end <= g.preview.source_length);
            let preview = g.preview;
            assert!(!g.update(f32::INFINITY, false));
            assert_eq!(g.preview, preview);
        }
        let step = MidiGrid::Sixteenth.step_raw().unwrap();
        let last_grid = u64::MAX / step * step;
        assert_eq!(
            clamp_loop_grid(u64::MAX, 1, u64::MAX, MidiGrid::Sixteenth),
            last_grid
        );
        assert_eq!(
            clamp_loop_grid(0, u64::MAX - 1, u64::MAX, MidiGrid::Sixteenth),
            u64::MAX - 1
        );
        assert_eq!(
            clamp_loop_grid(0, beat + 1, beat + 3, MidiGrid::Off),
            beat + 1
        );
        let mut off = Gesture {
            context: Context {
                grid: MidiGrid::Off,
                ..base
            },
            down_x: 100.0,
            handle: Handle::LoopStart,
            preview: base.values,
        };
        off.update(f32::MAX, false);
        assert_eq!(off.preview.loop_start, 4 * beat - 1);
        off.update(f32::MAX, true);
        assert_eq!(off.preview.loop_start, 4 * beat - beat / 4);
        let preview = off.preview;
        off.update(102.0, true);
        assert_eq!(off.preview, base.values); // The original three-pixel no-op threshold.
        assert_ne!(off.preview, preview);
    }
    #[test]
    fn loop_source_start_constraint_depends_on_enable_and_snap_override_is_exact() {
        let beat = hex_project::BeatTime::ONE.raw();
        let base = context();
        for looping in [false, true] {
            let c = Context {
                values: SourceValues {
                    source_start: beat * 5 / 2,
                    loop_start: beat,
                    loop_end: 3 * beat,
                    looping,
                    ..base.values
                },
                ..base
            };
            let mut g = Gesture {
                context: c,
                down_x: 400.0,
                handle: Handle::LoopEnd,
                preview: c.values,
            };
            g.update(-f32::MAX, false);
            assert_eq!(g.preview.loop_start, beat);
            assert_eq!(
                g.preview.loop_end,
                if looping { beat * 11 / 4 } else { beat * 5 / 4 }
            );
            let mut body = Gesture {
                context: c,
                down_x: 300.0,
                handle: Handle::LoopBody,
                preview: c.values,
            };
            body.update(-f32::MAX, false);
            assert_eq!(body.preview.loop_end - body.preview.loop_start, 2 * beat);
            assert_eq!(
                body.preview.loop_start,
                if looping { beat * 3 / 4 } else { 0 }
            );
        }
        let c = Context {
            values: SourceValues {
                loop_start: beat,
                loop_end: 3 * beat,
                ..base.values
            },
            ..base
        };
        let mut g = Gesture {
            context: c,
            down_x: 200.0,
            handle: Handle::LoopStart,
            preview: c.values,
        };
        g.update(235.0, false);
        assert_eq!(g.preview.loop_start, beat * 5 / 4);
        g.update(235.0, true);
        assert!((g.preview.loop_start as i128 - (beat as i128 * 135 / 100)).abs() <= 1);
        assert_eq!(g.preview.loop_end, 3 * beat);
        let preview = g.preview;
        assert!(!g.update(f32::NAN, false));
        assert_eq!(g.preview, preview);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_loop_ui_tests.rs"]
mod loop_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_time_selection_ui_tests.rs"]
mod time_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_ruler_capture_tests.rs"]
mod capture_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_time_selection_capture_tests.rs"]
mod time_capture_tests;
