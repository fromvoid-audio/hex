//! Source-local piano ruler, playback start and retained loop brace. Owns one
//! captured preview; the Window lease owns held input only. Release uses the
//! existing source edit command and Session ACK owner.
//! Source start is neither the insertion cursor nor the playback cursor. Blank
//! clicks delegate to the cue owner; drags retain source-time selection.
use super::{
    EditorView, midi_drag,
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
mod geometry;
use geometry::{Context, Gesture, Handle, clamp_loop_grid};

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
        let values = SourceValues::for_clip(&state.project, clip)?;
        let arrangement = state.project.midi_clip(clip);
        let context = Context {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            values,
            play_start_region: arrangement
                .is_some_and(|clip| !clip.looping() && clip.content_view().is_none()),
            arrangement_scope: arrangement.is_some(),
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
                        if view.midi_ruler.gesture.is_none() {
                            view.sync_held_play_start_audition();
                        }
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.owns_midi_ruler_capture(context, handle) {
                        view.midi_ruler.cancel();
                        view.sync_held_play_start_audition();
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
    pub(super) fn held_midi_play_start_audition(
        &self,
    ) -> Option<hex_session::service::ArrangementPlayStartAudition> {
        use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};
        let gesture = self.midi_ruler.gesture?;
        if gesture.handle != Handle::Start
            || gesture.preview == gesture.context.values
            || self.midi_ruler_context(gesture.context.bounds) != Some(gesture.context)
        {
            return None;
        }
        let command = midi_source_controls::edit_command(
            &self.state.as_ref()?.project,
            gesture.context.clip,
            gesture.preview,
        )?;
        Some(ArrangementPlayStartAudition {
            context: hex_engine::AudioSourceContext {
                project_generation: gesture.context.generation,
                editor_revision: gesture.context.revision,
            },
            edit: PlayStartEdit::from_midi_command(&command)?,
        })
    }

    fn update_midi_ruler(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) -> bool {
        let Some(gesture) = self.midi_ruler.gesture else {
            return false;
        };
        let changed = if !x.is_finite() || self.midi_ruler_context(bounds) != Some(gesture.context)
        {
            self.midi_ruler.cancel()
        } else {
            self.midi_ruler
                .gesture
                .as_mut()
                .is_some_and(|gesture| gesture.update(x, invert_snap))
        };
        if changed && gesture.handle == Handle::Start {
            self.sync_held_play_start_audition();
        }
        changed
    }
    fn finish_midi_ruler(&mut self, bounds: CanvasBounds, x: f32, invert_snap: bool) {
        let Some(mut gesture) = self.midi_ruler.gesture.take() else {
            return;
        };
        if !x.is_finite() || self.midi_ruler_context(bounds) != Some(gesture.context) {
            self.sync_held_play_start_audition();
            return;
        }
        gesture.update(x, invert_snap);
        if gesture.preview == gesture.context.values {
            self.sync_held_play_start_audition();
            return;
        }
        let play_start = gesture.handle == Handle::Start;
        if play_start {
            self.finish_held_play_start_audition();
        }
        self.commit_midi_clip_loop(gesture.context.clip, gesture.preview);
        if play_start {
            self.settle_released_play_start_audition(gesture.context.clip);
        }
    }
    pub(super) fn midi_ruler_held_preview_clip(
        &self,
        clip: &hex_project::MidiClip,
        revision: u64,
    ) -> Option<hex_project::MidiClip> {
        let g = self.midi_ruler.gesture.filter(|g| {
            g.context.clip == clip.id()
                && g.context.generation == self.project_generation
                && g.context.revision == revision
                && g.preview != g.context.values
        })?;
        match g.handle {
            Handle::Start => clip
                .with_play_start(hex_project::BeatTime::from_raw(g.preview.source_start))
                .ok(),
            Handle::LoopStart | Handle::LoopEnd | Handle::LoopBody => clip
                .with_scope_range(
                    hex_project::BeatTime::from_raw(g.preview.loop_start),
                    hex_project::BeatTime::from_raw(g.preview.loop_end),
                    g.preview.looping,
                )
                .ok(),
        }
    }

    pub(super) fn midi_header_loop_values(
        &self,
        clip: ClipId,
        revision: u64,
    ) -> Option<SourceValues> {
        self.midi_ruler
            .gesture
            .filter(|g| {
                g.context.clip == clip
                    && g.context.generation == self.project_generation
                    && g.context.revision == revision
                    && g.preview != g.context.values
            })
            .map(|g| g.preview)
            .or_else(|| self.pending_clip_loop_midi(clip))
    }
    pub(super) fn midi_display_play_start(&self) -> Option<u64> {
        self.midi_ruler
            .gesture
            .map(|gesture| gesture.preview.source_start)
            .or_else(|| {
                self.detail_clip_id()
                    .and_then(|id| self.pending_clip_loop_midi(id))
                    .map(|v| v.source_start)
            })
            .or_else(|| self.midi_clip().map(|source| source.source_start().raw()))
    }
    pub(super) fn midi_source_ruler(&mut self, cx: &mut ViewContext<Self>) -> Div {
        let start = self.piano_beat_start;
        let span = self.piano_beat_span.raw().max(1);
        let marker = self.midi_display_play_start();
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
        let step = super::midi_viewport_grid::ruler_step(
            span,
            self.midi_snap_grid
                .step_raw()
                .unwrap_or(hex_project::BeatTime::ONE.raw()),
        );
        let step = i128::from(step);
        let origin = i128::from(marker.unwrap_or(0));
        let relative_start = start.saturating_sub(origin);
        let first = -(-relative_start).div_euclid(step);
        for index in 0..=8_i128 {
            let Some(relative) = first
                .checked_add(index)
                .and_then(|value| value.checked_mul(step))
            else {
                break;
            };
            let Some(raw) = origin.checked_add(relative) else {
                break;
            };
            let Some(offset) = raw
                .checked_sub(start)
                .filter(|offset| *offset >= 0 && *offset <= i128::from(span))
            else {
                break;
            };
            let fraction = (offset as f64 / span as f64) as f32;
            let beats = relative as f64 / hex_project::BeatTime::ONE.raw() as f64;
            let beats = if beats == 0.0 { 0.0 } else { beats };
            ruler = ruler.children(super::timeline_strip::ruler_tick(
                fraction,
                format!("{beats:.2}b"),
                relative.rem_euclid(i128::from(super::midi_viewport_grid::ACCENT_PERIOD)) == 0,
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
