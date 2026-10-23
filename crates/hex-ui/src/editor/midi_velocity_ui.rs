//! Compact velocity-lane view. The pure model owns geometry/preview validation;
//! this adapter captures GPUI input and submits one canonical batch on release.
use super::{
    EditorView,
    midi_group_drag::{GroupNote, capture_selected_notes},
    midi_stroke::CanvasBounds,
    midi_velocity::{self, Context, Gesture, Scene, SceneKey},
};
use crate::theme;
use gpui::{
    Bounds, Context as ViewContext, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use hex_project::{Command, NoteId};
use hex_session::service::Request;
use std::sync::Arc;

pub(super) struct State {
    open: bool,
    height: f32,
    scene: Option<Arc<Scene>>,
    pub(super) gesture: Option<Gesture>,
    marquee: Option<super::midi_selection::Marquee>,
    resize: Option<(f32, f32)>,
    hover: Option<NoteId>,
    cycle: Option<(SceneKey, [f32; 2], NoteId)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: true,
            height: 64.0,
            scene: None,
            gesture: None,
            marquee: None,
            resize: None,
            hover: None,
            cycle: None,
        }
    }
}
impl State {
    pub(super) fn visible_height(&self) -> f32 {
        if self.open { self.height + 3.0 } else { 18.0 }
    }
    pub(super) fn cancel(&mut self) -> bool {
        let changed = self.marquee.take().is_some()
            | self.gesture.take().is_some()
            | self.resize.take().is_some()
            | self.hover.take().is_some();
        self.cycle = None;
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
    fn velocity_context(&self, bounds: CanvasBounds) -> Option<Context> {
        let state = self.state.as_ref()?;
        let clip = self.detail_midi_clip_data()?;
        Some(Context {
            scene: SceneKey {
                generation: self.project_generation,
                revision: state.revision,
                clip,
                start: self.piano_beat_start.raw(),
                span: self.piano_beat_span.raw(),
            },
            bounds,
            source_length: state.project.midi_source(clip)?.source_length().raw(),
            selection_epoch: self.midi_selection.epoch(),
        })
    }
    fn velocity_marquee_context(
        &self,
        bounds: CanvasBounds,
    ) -> Option<super::midi_selection::MarqueeContext> {
        let context = self.velocity_context(bounds)?;
        Some(super::midi_selection::MarqueeContext {
            scene: super::midi_scene::SceneKey {
                generation: context.scene.generation,
                revision: context.scene.revision,
                clip: context.scene.clip,
                beat_start: context.scene.start,
                beat_span: context.scene.span,
                pitch_low: self.piano_pitch_low,
                pitch_rows: self.piano_pitch_rows,
            },
            bounds,
            selection_epoch: context.selection_epoch,
        })
    }
    pub(super) fn velocity_marquee_active(&self) -> bool {
        self.midi_velocity.marquee.is_some()
    }
    pub(super) fn velocity_marquee_highlight(&self) -> Option<Arc<[NoteId]>> {
        let marquee = self.midi_velocity.marquee.as_ref()?;
        let captured = marquee.context();
        (self.midi_velocity.open
            && !self.edit_pending()
            && self.velocity_marquee_context(captured.bounds) == Some(captured))
        .then(|| marquee.highlighted())
    }
    pub(super) fn velocity_lane(&mut self, cx: &mut ViewContext<Self>) -> Option<Div> {
        let state = Arc::clone(self.state.as_ref()?);
        let clip = self.detail_midi_clip_data()?;
        let source = state.project.midi_source(clip)?;
        let key = SceneKey {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            start: self.piano_beat_start.raw(),
            span: self.piano_beat_span.raw(),
        };
        if self
            .midi_velocity
            .scene
            .as_ref()
            .is_none_or(|scene| scene.key != key)
        {
            self.midi_velocity.cancel();
            self.midi_velocity.cycle = None;
            match Scene::build(key, source.notes()) {
                Ok(scene) => self.midi_velocity.scene = Some(Arc::new(scene)),
                Err(error) => {
                    self.error = Some(error.into());
                    return None;
                }
            }
        }
        let scene = Arc::clone(self.midi_velocity.scene.as_ref()?);
        let label = self
            .midi_velocity
            .hover
            .and_then(|id| scene.points().iter().find(|p| p.note.id == id))
            .map_or_else(
                || "velocity".to_string(),
                |point| {
                    format!(
                        "p{} · v{} · ch{}",
                        point.note.pitch,
                        self.midi_velocity
                            .gesture
                            .as_ref()
                            .and_then(|g| g.preview(point.note.id))
                            .unwrap_or(point.note.velocity),
                        point.note.channel + 1
                    )
                },
            );
        let left = div()
            .w(px(228.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px_1()
            .text_xs()
            .child(
                div()
                    .id("midi-velocity-toggle")
                    .debug_selector(|| "midi-velocity-toggle".into())
                    .h(px(18.0))
                    .cursor_pointer()
                    .child(theme::text(if self.midi_velocity.open {
                        format!("▾ {label}")
                    } else {
                        "▸ velocity".to_string()
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _, cx| {
                            view.midi_velocity.open = !view.midi_velocity.open;
                            view.cancel_midi_note_gesture();
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .when(self.midi_velocity.open, |left| {
                let captured = self.velocity_random_context();
                let enabled = !self.edit_pending() && !self.midi_selection.is_empty();
                left.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .child(
                            div()
                                .id("midi-velocity-randomize")
                                .debug_selector(|| "midi-velocity-randomize".into())
                                .cursor_pointer()
                                .px_1()
                                .text_color(rgb(theme::color(if enabled {
                                    theme::PaletteRole::Text
                                } else {
                                    theme::PaletteRole::Muted
                                })))
                                .hover(|style| {
                                    style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
                                })
                                .tooltip(|_, cx| {
                                    cx.new(|_| super::transport::MiniTooltip {
                                        description:
                                            "randomize selected velocities only · one undo".into(),
                                    })
                                    .into()
                                })
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    if let Some(captured) = captured {
                                        view.randomize_selected_velocities(captured);
                                    }
                                    cx.notify();
                                }))
                                .child(theme::text("rnd")),
                        )
                        .child(
                            div()
                                .id("midi-velocity-random-less")
                                .debug_selector(|| "midi-velocity-random-less".into())
                                .cursor_pointer()
                                .px_1()
                                .child(theme::text("−"))
                                .tooltip(|_, cx| {
                                    cx.new(|_| super::transport::MiniTooltip {
                                        description:
                                            "reduce velocity randomization amount · minimum 0"
                                                .into(),
                                    })
                                    .into()
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.midi_velocity_randomize.amount =
                                        view.midi_velocity_randomize.amount.saturating_sub(8);
                                    cx.notify();
                                })),
                        )
                        .child(theme::text(format!(
                            "±{}",
                            self.midi_velocity_randomize.amount
                        )))
                        .child(
                            div()
                                .id("midi-velocity-random-more")
                                .debug_selector(|| "midi-velocity-random-more".into())
                                .cursor_pointer()
                                .px_1()
                                .child(theme::text("+"))
                                .tooltip(|_, cx| {
                                    cx.new(|_| super::transport::MiniTooltip {
                                        description:
                                            "increase velocity randomization amount · maximum 126"
                                                .into(),
                                    })
                                    .into()
                                })
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.midi_velocity_randomize.amount = view
                                        .midi_velocity_randomize
                                        .amount
                                        .saturating_add(8)
                                        .min(126);
                                    cx.notify();
                                })),
                        ),
                )
            });
        let mut row = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .h(px(if self.midi_velocity.open {
                self.midi_velocity.height
            } else {
                18.0
            }))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(left);
        if !self.midi_velocity.open {
            return Some(row);
        }
        let gesture = self.midi_velocity.gesture.clone();
        let marquee = self.midi_velocity.marquee.clone();
        let time_selection = self.midi_time_selection_bounds();
        let selection = self
            .velocity_marquee_highlight()
            .unwrap_or_else(|| self.midi_selection.shared_ids());
        let selection_epoch = self.midi_selection.epoch();
        let source_length = source.source_length().raw();
        let entity = cx.entity();
        let paint_scene = Arc::clone(&scene);
        let event_scene = Arc::clone(&scene);
        let content = div()
            .id("midi-velocity-lane")
            .debug_selector(|| "midi-velocity-lane".into())
            .relative()
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .track_focus(&self.midi_focus)
            .key_context("hex_midi")
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        let captured = Context {
                            scene: key,
                            bounds: bounds_of(bounds),
                            source_length,
                            selection_epoch,
                        };
                        if !captured.bounds.is_valid() {
                            return;
                        }
                        let width = captured.bounds.width;
                        let height = captured.bounds.height;
                        let gesture = gesture
                            .as_ref()
                            .filter(|gesture| gesture.is_current(captured));
                        let marquee = marquee.as_ref().filter(|m| {
                            let context = m.context();
                            context.bounds == captured.bounds
                                && context.selection_epoch == selection_epoch
                                && context.scene.generation == key.generation
                                && context.scene.revision == key.revision
                                && context.scene.clip == key.clip
                                && context.scene.beat_start == key.start
                                && context.scene.beat_span == key.span
                        });
                        let pointer = window.mouse_position();
                        let hovered = hitbox
                            .is_hovered(window)
                            .then(|| {
                                paint_scene.hit(
                                    [
                                        f32::from(pointer.x - bounds.origin.x),
                                        f32::from(pointer.y - bounds.origin.y),
                                    ],
                                    [width, height],
                                    None,
                                )
                            })
                            .flatten();
                        window.set_cursor_style(
                            if gesture.is_some() || hovered.is_some() {
                                gpui::CursorStyle::ResizeUpDown
                            } else if marquee.is_some() || window.modifiers().shift {
                                gpui::CursorStyle::Crosshair
                            } else {
                                gpui::CursorStyle::IBeam
                            },
                            &hitbox,
                        );
                        if let Some((first, last)) = time_selection {
                            let first = first.max(key.start);
                            let last = last.min(key.start.saturating_add(key.span));
                            if first < last {
                                let left =
                                    ((first - key.start) as f64 / key.span as f64) as f32 * width;
                                let right =
                                    ((last - key.start) as f64 / key.span as f64) as f32 * width;
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(bounds.origin.x + px(left), bounds.origin.y),
                                        size(px(right - left), bounds.size.height),
                                    ),
                                    gpui::Hsla::from(rgb(theme::color(
                                        theme::PaletteRole::Secondary,
                                    )))
                                    .opacity(theme::TIME_SELECTION_OPACITY),
                                ));
                            }
                        }
                        let mut quad = |x: f32, y: f32, w: f32, h: f32, color: u32| {
                            if w > 0.0 && h > 0.0 {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                        size(px(w), px(h)),
                                    ),
                                    rgb(color),
                                ));
                            }
                        };
                        for velocity in [1, 64, 127] {
                            if let Some(y) = midi_velocity::handle_y(velocity, height) {
                                quad(0.0, y, width, 1.0, theme::color(theme::PaletteRole::Border));
                            }
                        }
                        for point in
                            paint_scene
                                .points()
                                .iter()
                                .filter(|point| selection.binary_search(&point.note.id).is_err())
                                .chain(paint_scene.points().iter().filter(|point| {
                                    selection.binary_search(&point.note.id).is_ok()
                                }))
                        {
                            let selected = selection.binary_search(&point.note.id).is_ok();
                            let velocity = gesture
                                .and_then(|gesture| gesture.preview(point.note.id))
                                .unwrap_or(point.note.velocity);
                            let Some(y) = midi_velocity::handle_y(velocity, height) else {
                                continue;
                            };
                            let x = point.x * width;
                            let color = theme::color(if selected {
                                theme::PaletteRole::Primary
                            } else {
                                theme::PaletteRole::Muted
                            });
                            quad(x, y, 1.0, (height - 4.0 - y).max(1.0), color);
                            quad(
                                (x - 3.0).max(0.0),
                                y - 2.0,
                                6.0_f32.min(width - x + 3.0),
                                4.0,
                                color,
                            );
                            if hovered == Some(point.note.id) {
                                quad(
                                    (x - 4.0).max(0.0),
                                    y - 3.0,
                                    8.0_f32.min(width - x + 4.0),
                                    1.0,
                                    theme::color(theme::PaletteRole::Text),
                                );
                            }
                        }
                        if let Some(marquee) = marquee {
                            let (first, last) = marquee.rectangle();
                            let left = first[0].min(last[0]).clamp(0.0, width);
                            let top = first[1].min(last[1]).clamp(0.0, height);
                            let right = first[0].max(last[0]).clamp(0.0, width);
                            let bottom = first[1].max(last[1]).clamp(0.0, height);
                            let color = theme::color(theme::PaletteRole::Primary);
                            quad(left, top, (right - left).max(1.0), 1.0, color);
                            quad(left, bottom - 1.0, (right - left).max(1.0), 1.0, color);
                            quad(left, top, 1.0, (bottom - top).max(1.0), color);
                            quad(right - 1.0, top, 1.0, (bottom - top).max(1.0), color);
                        }
                        let wheel = entity.clone();
                        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                            if !phase.capture() || !bounds.contains(&event.position) {
                                return;
                            }
                            let delta = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                            if !delta.is_finite() || delta == 0.0 {
                                return;
                            }
                            cx.stop_propagation();
                            wheel.update(cx, |view, cx| {
                                if view.zoom_piano_wheel_at(
                                    [captured.bounds.x, captured.bounds.y, width, height],
                                    f32::from(event.position.x),
                                    delta,
                                ) {
                                    cx.notify();
                                }
                            });
                        });
                        let down = entity.clone();
                        let down_scene = Arc::clone(&event_scene);
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if !phase.bubble()
                                || event.button != MouseButton::Left
                                || !bounds.contains(&event.position)
                            {
                                return;
                            }
                            down.update(cx, |view, cx| {
                                if view.edit_pending()
                                    || view.context_menu.is_some()
                                    || view
                                        .velocity_context(captured.bounds)
                                        .is_none_or(|current| current != captured)
                                {
                                    return;
                                }
                                let position = [
                                    f32::from(event.position.x - bounds.origin.x),
                                    f32::from(event.position.y - bounds.origin.y),
                                ];
                                let after = view
                                    .midi_velocity
                                    .cycle
                                    .filter(|(previous, at, _)| {
                                        *previous == key
                                            && (at[0] - position[0]).abs() <= 3.0
                                            && (at[1] - position[1]).abs() <= 3.0
                                    })
                                    .map(|(_, _, id)| id);
                                let Some(id) = down_scene.hit(position, [width, height], after)
                                else {
                                    view.cancel_midi_note_gesture();
                                    view.midi_focus.focus(window);
                                    if event.modifiers.shift {
                                        view.clear_midi_time_selection();
                                        view.midi_velocity.marquee = view
                                            .velocity_marquee_context(captured.bounds)
                                            .and_then(|context| {
                                                super::midi_selection::Marquee::begin(
                                                    context,
                                                    position,
                                                    true,
                                                    &view.midi_selection,
                                                )
                                            });
                                    } else {
                                        view.begin_midi_velocity_time_selection(
                                            captured.bounds,
                                            f32::from(event.position.x),
                                            event.modifiers.platform,
                                        );
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                };
                                view.cancel_midi_note_gesture();
                                view.midi_focus.focus(window);
                                view.midi_velocity.cycle = Some((key, position, id));
                                view.midi_velocity.hover = Some(id);
                                if event.modifiers.shift {
                                    if let Err(error) = view.midi_selection.toggle(id) {
                                        view.error = Some(error.into());
                                        return;
                                    }
                                } else if !view.midi_selection.contains(id) {
                                    view.midi_selection.set_single(id);
                                }
                                view.clear_midi_time_selection();
                                view.audition_midi_selection();
                                if view.midi_selection.contains(id) {
                                    let Some(context) = view.velocity_context(captured.bounds)
                                    else {
                                        return;
                                    };
                                    let selected = view.midi_selection.shared_ids();
                                    let originals = view
                                        .state
                                        .as_ref()
                                        .and_then(|state| state.project.midi_source(clip))
                                        .map(|source| {
                                            capture_selected_notes(
                                                source.notes().iter().map(|note| GroupNote {
                                                    id: note.id(),
                                                    start: note.start().raw(),
                                                    duration: note.duration().raw(),
                                                    pitch: note.pitch(),
                                                    channel: note.channel(),
                                                    velocity: note.velocity(),
                                                }),
                                                &selected,
                                            )
                                        });
                                    if let Some(originals) = originals {
                                        match originals.and_then(|notes| {
                                            Gesture::begin(
                                                context,
                                                notes,
                                                f32::from(event.position.y),
                                            )
                                        }) {
                                            Ok(gesture) => {
                                                view.midi_velocity.gesture = Some(gesture)
                                            }
                                            Err(error) => view.error = Some(error.into()),
                                        }
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                        let movement = entity.clone();
                        let hover_scene = Arc::clone(&event_scene);
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if !event.dragging() {
                                if !phase.capture() {
                                    return;
                                }
                                movement.update(cx, |view, cx| {
                                    if view.midi_velocity_time_selection_active() {
                                        view.midi_time_selection.cancel();
                                        cx.notify();
                                    }
                                    if view.midi_velocity.marquee.take().is_some() {
                                        cx.notify();
                                    }
                                    let hover = if bounds.contains(&event.position)
                                        && !view.edit_pending()
                                    {
                                        hover_scene.hit(
                                            [
                                                f32::from(event.position.x - bounds.origin.x),
                                                f32::from(event.position.y - bounds.origin.y),
                                            ],
                                            [width, height],
                                            None,
                                        )
                                    } else {
                                        None
                                    };
                                    if view.midi_velocity.hover != hover {
                                        view.midi_velocity.hover = hover;
                                        cx.notify();
                                    }
                                });
                                return;
                            }
                            if !phase.bubble() {
                                return;
                            }
                            movement.update(cx, |view, cx| {
                                if view.midi_velocity_time_selection_active() {
                                    if event.pressed_button == Some(MouseButton::Left) {
                                        view.update_midi_time_selection(
                                            captured.bounds,
                                            f32::from(event.position.x),
                                            event.modifiers.platform,
                                        );
                                    } else {
                                        view.midi_time_selection.cancel();
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                if let Some(mut marquee) = view.midi_velocity.marquee.take() {
                                    if event.pressed_button == Some(MouseButton::Left)
                                        && let Some(context) = view
                                            .velocity_marquee_context(captured.bounds)
                                            .filter(|_| !view.edit_pending())
                                    {
                                        let last = [
                                            f32::from(event.position.x - bounds.origin.x),
                                            f32::from(event.position.y - bounds.origin.y),
                                        ];
                                        match marquee.update_with_hits(
                                            context,
                                            &view.midi_selection,
                                            last,
                                            |first, last| {
                                                hover_scene.marquee_hits(first, last, width, height)
                                            },
                                        ) {
                                            Ok(_) => view.midi_velocity.marquee = Some(marquee),
                                            Err(error) => view.error = Some(error.into()),
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                if let Some((start, initial)) = view.midi_velocity.resize {
                                    let next = (initial + start - f32::from(event.position.y))
                                        .clamp(40.0, 128.0);
                                    if next.is_finite() {
                                        view.midi_velocity.height = next;
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                    return;
                                }
                                let Some(mut gesture) = view.midi_velocity.gesture.take() else {
                                    return;
                                };
                                let current = view
                                    .velocity_context(captured.bounds)
                                    .filter(|_| !view.edit_pending());
                                if let Some(current) = current {
                                    match gesture.update(current, f32::from(event.position.y)) {
                                        Ok(_) => view.midi_velocity.gesture = Some(gesture),
                                        Err(error) => view.error = Some(error.into()),
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                        let release = entity.clone();
                        let release_scene = Arc::clone(&event_scene);
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if !phase.bubble() || event.button != MouseButton::Left {
                                return;
                            }
                            release.update(cx, |view, cx| {
                                if view.midi_velocity_time_selection_active() {
                                    view.finish_midi_time_selection(
                                        captured.bounds,
                                        f32::from(event.position.x),
                                        event.modifiers.platform,
                                    );
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                if let Some(mut marquee) = view.midi_velocity.marquee.take() {
                                    if let Some(context) = view
                                        .velocity_marquee_context(captured.bounds)
                                        .filter(|_| !view.edit_pending())
                                    {
                                        let last = [
                                            f32::from(event.position.x - bounds.origin.x),
                                            f32::from(event.position.y - bounds.origin.y),
                                        ];
                                        let result = marquee
                                            .update_with_hits(
                                                context,
                                                &view.midi_selection,
                                                last,
                                                |first, last| {
                                                    release_scene
                                                        .marquee_hits(first, last, width, height)
                                                },
                                            )
                                            .and_then(|_| {
                                                marquee.finish(context, &mut view.midi_selection)
                                            });
                                        match result {
                                            Ok(_) => view.audition_midi_selection(),
                                            Err(error) => view.error = Some(error.into()),
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                if view.midi_velocity.resize.take().is_some() {
                                    cx.stop_propagation();
                                    cx.notify();
                                    return;
                                }
                                let Some(mut gesture) = view.midi_velocity.gesture.take() else {
                                    return;
                                };
                                if let Some(current) = view
                                    .velocity_context(captured.bounds)
                                    .filter(|_| !view.edit_pending())
                                {
                                    match gesture
                                        .update(current, f32::from(event.position.y))
                                        .and_then(|_| gesture.updates(current))
                                    {
                                        Ok(Some(notes)) => {
                                            view.enqueue(Request::Edit(Command::UpdateMidiNotes {
                                                clip,
                                                notes,
                                            }));
                                        }
                                        Ok(None) => {}
                                        Err(error) => view.error = Some(error.into()),
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        row = row.child(content);
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(
                    div()
                        .id("midi-velocity-resize")
                        .debug_selector(|| "midi-velocity-resize".into())
                        .h(px(3.0))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .bg(rgb(theme::color(theme::PaletteRole::Border)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &MouseDownEvent, _, cx| {
                                view.cancel_midi_note_gesture();
                                view.midi_velocity.resize =
                                    Some((f32::from(event.position.y), view.midi_velocity.height));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
                .child(row),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_velocity_ui_tests.rs"]
mod tests;
