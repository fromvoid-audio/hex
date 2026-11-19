//! Compact piano-roll Canvas rendering and pointer interaction.
//!
//! Wheel navigation changes horizontal time only; a vertical drag on the key
//! gutter changes the centered pitch span (1–128 rows). Both are view-only.
//! Middle-drag or explicit pan mode moves both axes without editing.
//! Every paint, hit and captured edit context carries the same pitch span.
//! Passive hover reuses click/resize hits and never changes musical selection.
#[path = "midi_canvas/geometry.rs"]
mod geometry;
#[path = "midi_note_pointer.rs"]
mod note_pointer;

use super::midi_drag::{self, NotePreview};
use super::midi_draw::StrokeModifiers;
use super::{EditorView, midi::NoteField};
use crate::theme;
use geometry::{beat_at_x, beat_label, pitch_at_y, source_beat_label};
use gpui::{
    Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, ScrollWheelEvent, Window,
    canvas, div, fill, point, prelude::*, px, relative, rgb, size,
};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

fn gesture_bounds_from(bounds: Bounds<gpui::Pixels>) -> [f32; 4] {
    [
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    ]
}

impl EditorView {
    pub(super) fn midi_editor(
        &mut self,
        _window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let state = std::sync::Arc::clone(self.state.as_ref()?);
        let clip_id = self.detail_clip_id()?;
        let source = state.project.midi_source(clip_id)?;
        let clip_len = source.source_length();
        let owner = (self.project_generation, clip_id);
        if self.piano_view_clip != Some(owner) {
            self.zoom_fit.clear_clip();
            self.cancel_midi_note_gesture();
            self.clear_midi_time_selection();
            self.piano_view_clip = Some(owner);
            self.midi_marker = Some(BeatTime::ZERO.raw());
            self.cancel_midi_audition();
            self.midi_step.cancel_pending();
            if self.midi_step.enabled {
                self.cancel_computer_keyboard();
            }
            self.piano_zoom_wheel_accum = 0.0;
            self.piano_pitch_wheel_accum = 0.0;
            self.piano_beat_start = 0;
            self.piano_beat_span = BeatTime::from_raw(super::midi_scene::fit_span(
                clip_len.raw(),
                BeatTime::ONE.raw(),
            ));
        }
        let arrangement = state.project.midi_clip(clip_id);
        let revision = state.revision;
        let generation = self.project_generation;
        let source_values = super::midi_source_controls::SourceValues::from_source(
            source,
            arrangement.map_or(0, |clip| clip.start().raw()),
            arrangement.map_or(source.source_length().raw(), |clip| {
                clip.arrangement_length().raw()
            }),
        );
        let beat_start = self.piano_beat_start;
        let beat_span = self.piano_beat_span;
        let pitch_low = self.piano_pitch_low;
        let pitch_rows = self.piano_pitch_rows;
        let grid = self.midi_snap_grid;
        let notes = source.notes();
        let key = super::midi_scene::SceneKey {
            generation,
            revision,
            clip: clip_id,
            beat_start,
            beat_span: beat_span.raw(),
            pitch_low,
            pitch_rows,
        };
        let scene = self.midi_scene_cache.get_or_build(key, notes).0;
        let draw_overwrite = match self.midi_overwrite_preview() {
            Ok(preview) => preview,
            Err(error) => {
                self.error = Some(error.to_string().into());
                self.cancel_midi_note_gesture();
                self.retire_pending_midi_drag();
                None
            }
        };
        let selected = if self.midi_selection.len() == 1 {
            self.midi_selection.primary()
        } else {
            None
        };
        let group_selected = self.midi_selection.len() > 1;
        let selected_ids = self
            .velocity_marquee_highlight()
            .unwrap_or_else(|| self.midi_selection.shared_ids());
        let draw_primary = self.midi_selection.primary();
        let selection_epoch = self.midi_selection.epoch();
        let displayed_note = self.displayed_midi_note_gesture();
        let selected_note_data = selected.and_then(|id| {
            if let Some(gesture) = displayed_note.filter(|gesture| gesture.note == id) {
                return Some((
                    gesture.preview.pitch,
                    i128::from(gesture.preview.start) - i128::from(gesture.source_prefix),
                    gesture.preview.duration,
                    gesture.velocity,
                ));
            }
            if let Some(note) = scene.note(id) {
                return Some((
                    note.pitch,
                    i128::from(note.start),
                    note.duration,
                    note.velocity,
                ));
            }
            notes.iter().find(|note| note.id() == id).map(|note| {
                (
                    note.pitch(),
                    i128::from(note.start().raw()),
                    note.duration().raw(),
                    note.velocity(),
                )
            })
        });
        let status = if let Some((first, last)) = self.midi_time_selection_bounds() {
            format!(
                "time {}–{}b · cmd+d duplicate",
                beat_label(first),
                beat_label(last)
            )
        } else if self.midi_selection.len() > 1 {
            format!(
                "{} notes selected · drag to move · option-drag/cmd+d copy · backspace to delete · cmd+a select all",
                self.midi_selection.len()
            )
        } else {
            selected_note_data.map_or_else(
                || {
                    format!(
                        "{} beats · drag: time · shift-drag: notes · double-click: add",
                        beat_label(beat_span.raw())
                    )
                },
                |(pitch, start, duration, velocity)| {
                    format!(
                        "p{pitch} · t{} · g{} · v{}",
                        beat_label(start),
                        beat_label(duration),
                        velocity
                    )
                },
            )
        };

        let canvas_scene = std::sync::Arc::clone(&scene);
        let draw_selected = selected_ids;
        let draw_preview =
            displayed_note.map(|gesture| (gesture.note, gesture.preview, gesture.source_prefix));
        let draw_group = self
            .displayed_midi_group_gesture()
            .map(|gesture| (gesture.shared_originals(), gesture.clone()));
        let draw_stroke = self.displayed_midi_stroke_preview();
        let draw_creation = self.displayed_midi_creation();
        let draw_marquee = self.midi_marquee.clone();
        let draw_marker = self.midi_marker;
        let draw_step = self.midi_step_preview();
        let draw_playhead = self.midi_playhead_source_beat();
        let draw_time = self.midi_time_selection_bounds();
        let draw_pitch_locked = self.midi_draw_pitch_locked;
        let draw_mode = self.midi_draw_mode;
        let draw_hit_cycle = self.midi_hit_cycle;
        let drag_cursor = if self.midi_pan.gesture.is_some() {
            Some(gpui::CursorStyle::ClosedHand)
        } else if self.midi_pan.enabled {
            Some(gpui::CursorStyle::OpenHand)
        } else {
            None
        }
        .or_else(|| {
            self.midi_note_gesture
                .map(|gesture| match gesture.kind {
                    midi_drag::GestureKind::Move { .. } => gpui::CursorStyle::ClosedHand,
                    midi_drag::GestureKind::ResizeLeading { .. } => gpui::CursorStyle::ResizeLeft,
                    midi_drag::GestureKind::ResizeTrailing { .. } => gpui::CursorStyle::ResizeRight,
                })
                .or_else(|| {
                    self.midi_group_gesture
                        .as_ref()
                        .map(|gesture| match gesture.kind() {
                            midi_drag::HitKind::Move => {
                                if gesture.is_copy() {
                                    gpui::CursorStyle::DragCopy
                                } else {
                                    gpui::CursorStyle::ClosedHand
                                }
                            }
                            midi_drag::HitKind::ResizeLeading => gpui::CursorStyle::ResizeLeft,
                            midi_drag::HitKind::ResizeTrailing => gpui::CursorStyle::ResizeRight,
                        })
                })
                .or_else(|| {
                    self.midi_stroke
                        .as_ref()
                        .map(|_| gpui::CursorStyle::Crosshair)
                })
        });
        let hover_allowed = self.context_menu.is_none()
            && !self.edit_pending()
            && draw_preview.is_none()
            && draw_group.is_none()
            && draw_stroke.is_none()
            && draw_creation.is_none()
            && draw_marquee.is_none()
            && !self.velocity_marquee_active()
            && self.midi_time_selection.gesture.is_none()
            && self.piano_pitch_zoom.is_none()
            && !self.midi_pan.enabled
            && self.midi_pan.gesture.is_none();
        let mut roll = div()
            .id("midi-note-roll")
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .track_focus(&self.midi_focus)
            .key_context("hex_midi")
            .debug_selector(|| "midi-note-roll".into());
        let entity = cx.entity();
        let canvas_scene_events = std::sync::Arc::clone(&scene);
        roll = roll.children([canvas(
            |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
            move |bounds, hitbox, window, cx| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                if width <= 0.0 || height <= 0.0 {
                    return;
                }
                entity.read(cx).renew_midi_marquee_pointer(
                    false,
                    super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    clip_id,
                    entity.entity_id(),
                    window,
                );
                entity.read(cx).renew_midi_note_pointer(
                    super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    clip_id,
                    entity.entity_id(),
                    window,
                );
                entity.read(cx).renew_midi_time_selection_capture(
                    super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    clip_id,
                    entity.entity_id(),
                    window,
                );
                entity.read(cx).renew_piano_pan_capture(
                    super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    clip_id,
                    entity.entity_id(),
                    window,
                );
                entity.read(cx).renew_midi_creation_capture(
                    super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    clip_id,
                    entity.entity_id(),
                    window,
                );
                let marquee_context = super::midi_selection::MarqueeContext {
                    scene: canvas_scene.key(),
                    bounds: super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    selection_epoch,
                };
                let marquee = draw_marquee
                    .as_ref()
                    .filter(|m| m.is_current(marquee_context));
                let draw_selected = marquee.map_or(draw_selected, |m| m.highlighted());
                let draw_marquee = marquee.map(|m| m.rectangle());
                let pointer = window.mouse_position();
                let hovered = if hover_allowed && hitbox.is_hovered(window) {
                    canvas_scene.hover(
                        f32::from(pointer.x - bounds.origin.x),
                        f32::from(pointer.y - bounds.origin.y),
                        width,
                        height,
                        draw_hit_cycle,
                        &draw_selected,
                    )
                } else {
                    None
                };
                if let Some(cursor) = drag_cursor {
                    window.set_cursor_style(cursor, &hitbox);
                } else if let Some(target) = hovered {
                    let cursor = if draw_mode {
                        gpui::CursorStyle::Crosshair
                    } else {
                        match target.kind {
                            midi_drag::HitKind::Move => gpui::CursorStyle::OpenHand,
                            midi_drag::HitKind::ResizeLeading => gpui::CursorStyle::ResizeLeft,
                            midi_drag::HitKind::ResizeTrailing => gpui::CursorStyle::ResizeRight,
                        }
                    };
                    window.set_cursor_style(cursor, &hitbox);
                }
                if let Some((first, last)) = draw_time {
                    let end = beat_start.saturating_add(i128::from(beat_span.raw()));
                    let first = i128::from(first).max(beat_start);
                    let last = i128::from(last).min(end);
                    if first < last {
                        let left =
                            ((first - beat_start) as f64 / beat_span.raw() as f64) as f32 * width;
                        let right =
                            ((last - beat_start) as f64 / beat_span.raw() as f64) as f32 * width;
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(left), bounds.origin.y),
                                size(px(right - left), bounds.size.height),
                            ),
                            gpui::Hsla::from(rgb(theme::time_selection_fill()))
                                .opacity(theme::TIME_SELECTION_OPACITY),
                        ));
                    }
                }
                let mut quad = |x: f32, y: f32, w: f32, h: f32, color: u32| {
                    let left = x.max(0.0);
                    let top = y.max(0.0);
                    let right = (x + w).min(width);
                    let bottom = (y + h).min(height);
                    if right > left && bottom > top {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(left), bounds.origin.y + px(top)),
                                size(px(right - left), px(bottom - top)),
                            ),
                            rgb(color),
                        ));
                    }
                };
                for row in 0..=usize::from(pitch_rows) {
                    let y = row as f32 * height / usize::from(pitch_rows) as f32;
                    quad(
                        0.0,
                        y,
                        width,
                        1.0,
                        if row % 6 == 0 {
                            theme::color(theme::PaletteRole::Border)
                        } else {
                            theme::color(theme::PaletteRole::Grid)
                        },
                    );
                }
                let raw_span = beat_span.raw();
                let step = grid.step_raw().unwrap_or(BeatTime::ONE.raw()).max(1);
                if let Some(ticks) =
                    super::midi_viewport_grid::ticks(beat_start, raw_span, step, 256)
                {
                    let grid_step = i128::from(step);
                    for index in 0..256_i128 {
                        let beat = ticks.first + index * i128::from(ticks.stride);
                        if beat > ticks.last {
                            break;
                        }
                        let raw = beat.saturating_mul(grid_step);
                        let x = (raw.saturating_sub(beat_start) as f64 / raw_span as f64) as f32
                            * width;
                        quad(
                            x,
                            0.0,
                            1.0,
                            height,
                            if raw % i128::from(super::midi_viewport_grid::ACCENT_PERIOD) == 0 {
                                theme::color(theme::PaletteRole::Border)
                            } else {
                                theme::color(theme::PaletteRole::Grid)
                            },
                        );
                    }
                }
                for note in canvas_scene.notes() {
                    if draw_selected.binary_search(&note.id).is_ok()
                        || draw_stroke
                            .as_ref()
                            .is_some_and(|stroke| stroke.removed.binary_search(&note.id).is_ok())
                        || draw_preview.is_some_and(|(id, _, _)| id == note.id)
                        || draw_group.as_ref().is_some_and(|(originals, gesture)| {
                            !gesture.is_copy()
                                && originals
                                    .binary_search_by_key(&note.id, |item| item.id)
                                    .is_ok()
                        })
                    {
                        continue;
                    }
                    let Some(note) = super::midi_note_overwrite::retained_geometry(
                        &canvas_scene,
                        *note,
                        draw_overwrite.as_ref(),
                    ) else {
                        continue;
                    };
                    let Some((x, right)) = super::midi_scene::pixel_bounds(note, width) else {
                        continue;
                    };
                    let y = note.y * height;
                    let w = right - x;
                    quad(
                        x,
                        y + 1.0,
                        w,
                        (height / f32::from(pitch_rows) - 2.0).max(1.0),
                        theme::color(if note.active {
                            theme::PaletteRole::Secondary
                        } else {
                            theme::PaletteRole::Muted
                        }),
                    );
                    let note_height = (height / f32::from(pitch_rows) - 2.0).max(1.0);
                    let border = theme::color(theme::PaletteRole::Background);
                    quad(x, y + 1.0, w, 1.0, border);
                    quad(x, y + note_height, w, 1.0, border);
                    if w >= 3.0 {
                        if note.leading_edge_visible {
                            quad(x, y + 1.0, 1.0, note_height, border);
                        }
                        if note.trailing_edge_visible {
                            quad(right - 1.0, y + 1.0, 1.0, note_height, border);
                        }
                    }
                }
                for id in draw_selected.iter().copied() {
                    if draw_preview.is_some_and(|(preview_id, _, _)| preview_id == id)
                        || draw_stroke
                            .as_ref()
                            .is_some_and(|stroke| stroke.removed.binary_search(&id).is_ok())
                        || draw_group.as_ref().is_some_and(|(originals, gesture)| {
                            !gesture.is_copy()
                                && originals.binary_search_by_key(&id, |item| item.id).is_ok()
                        })
                    {
                        continue;
                    }
                    let Some(note) = canvas_scene.note(id) else {
                        continue;
                    };
                    let Some(note) = super::midi_note_overwrite::retained_geometry(
                        &canvas_scene,
                        note,
                        draw_overwrite.as_ref(),
                    ) else {
                        continue;
                    };
                    let Some((x, right)) = super::midi_scene::pixel_bounds(note, width) else {
                        continue;
                    };
                    let y = note.y * height;
                    let w = right - x;
                    quad(
                        x,
                        y + 1.0,
                        w,
                        (height / f32::from(pitch_rows) - 2.0).max(1.0),
                        theme::color(if note.active {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Muted
                        }),
                    );
                    if draw_primary == Some(id) {
                        let note_height = (height / f32::from(pitch_rows) - 2.0).max(1.0);
                        let accent = 2.0_f32.min(note_height);
                        let color = theme::color(theme::PaletteRole::Text);
                        quad(x, y + 1.0, w, accent, color);
                        quad(x, y + 1.0 + note_height - accent, w, accent, color);
                    }
                    if w >= 3.0 && note.leading_edge_visible {
                        quad(
                            x,
                            y + 1.0,
                            1.0,
                            (height / f32::from(pitch_rows) - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                    if w >= 3.0 && note.trailing_edge_visible {
                        quad(
                            x + w - 1.0,
                            y + 1.0,
                            1.0,
                            (height / f32::from(pitch_rows) - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                }

                for (index, mut note) in draw_stroke
                    .iter()
                    .flat_map(|stroke| stroke.inserted.iter().copied())
                    .chain(draw_creation)
                    .enumerate()
                {
                    let Some(duration) = draw_overwrite
                        .as_ref()
                        .map_or(Some(note.duration), |preview| {
                            preview.authored_duration(index)
                        })
                    else {
                        continue;
                    };
                    note.duration = duration;
                    if let Some(rect) = canvas_scene.insert_geometry(note) {
                        let x = rect.x * width;
                        let right = (rect.x + rect.width) * width;
                        quad(
                            x,
                            rect.y * height + 1.0,
                            right - x,
                            (height / f32::from(pitch_rows) - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Primary),
                        );
                    }
                }
                if let Some((id, item, prefix)) = draw_preview
                    && let Some(note) = canvas_scene.rebased_preview_geometry(id, item, prefix)
                    && let Some((x, right)) = super::midi_scene::pixel_bounds(note, width)
                {
                    quad(
                        x,
                        note.y * height + 1.0,
                        right - x,
                        (height / f32::from(pitch_rows) - 2.0).max(1.0),
                        theme::color(if canvas_scene.note(id).is_some_and(|n| !n.active) {
                            theme::PaletteRole::Muted
                        } else {
                            theme::PaletteRole::Primary
                        }),
                    );
                }

                if let Some((originals, gesture)) = &draw_group {
                    for (index, original) in originals.iter().enumerate() {
                        let Some(mut preview) = gesture.preview(original.id) else {
                            continue;
                        };
                        let Some(duration) = (if gesture.source_prefix() > 0 {
                            Some(BeatTime::from_raw(preview.duration))
                        } else {
                            draw_overwrite
                                .as_ref()
                                .map_or(Some(BeatTime::from_raw(preview.duration)), |plan| {
                                    plan.authored_duration(index)
                                })
                        }) else {
                            continue;
                        };
                        preview.duration = duration.raw();
                        let Some(note) = canvas_scene.rebased_preview_geometry(
                            original.id,
                            NotePreview {
                                start: preview.start,
                                duration: preview.duration,
                                pitch: preview.pitch,
                            },
                            gesture.source_prefix(),
                        ) else {
                            continue;
                        };
                        if let Some((x, right)) = super::midi_scene::pixel_bounds(note, width) {
                            quad(
                                x,
                                note.y * height + 1.0,
                                right - x,
                                (height / f32::from(pitch_rows) - 2.0).max(1.0),
                                theme::color(if original.active {
                                    theme::PaletteRole::Primary
                                } else {
                                    theme::PaletteRole::Muted
                                }),
                            );
                        }
                    }
                }

                if let Some(target) = hovered
                    && draw_selected.binary_search(&target.note).is_err()
                    && let Some(note) = canvas_scene.note(target.note)
                    && let Some((left, right)) = super::midi_scene::pixel_bounds(note, width)
                {
                    let top = note.y * height + 1.0;
                    let note_height = (height / f32::from(pitch_rows) - 2.0).max(1.0);
                    let color = theme::color(theme::PaletteRole::SecondaryText);
                    quad(left, top, right - left, 1.0, color);
                    quad(left, top + note_height - 1.0, right - left, 1.0, color);
                    quad(left, top, 1.0, note_height, color);
                    quad(right - 1.0, top, 1.0, note_height, color);
                    let edge = match target.kind {
                        midi_drag::HitKind::Move => None,
                        midi_drag::HitKind::ResizeLeading => Some(left),
                        midi_drag::HitKind::ResizeTrailing => {
                            Some(right - 3.0_f32.min(right - left))
                        }
                    };
                    if let Some(edge) = edge {
                        quad(
                            edge,
                            top,
                            3.0_f32.min(right - left),
                            note_height,
                            theme::color(theme::PaletteRole::SecondaryText),
                        );
                    }
                }
                if let Some((first, last)) = draw_marquee {
                    let left = first[0].min(last[0]);
                    let top = first[1].min(last[1]);
                    let width = (first[0] - last[0]).abs();
                    let height = (first[1] - last[1]).abs();
                    quad(
                        left,
                        top,
                        width.max(1.0),
                        1.0,
                        theme::color(theme::PaletteRole::Primary),
                    );
                    quad(
                        left,
                        top + height,
                        width.max(1.0),
                        1.0,
                        theme::color(theme::PaletteRole::Primary),
                    );
                    quad(
                        left,
                        top,
                        1.0,
                        height.max(1.0),
                        theme::color(theme::PaletteRole::Primary),
                    );
                    quad(
                        left + width,
                        top,
                        1.0,
                        height.max(1.0),
                        theme::color(theme::PaletteRole::Primary),
                    );
                }

                // Held chords are lines, separate from authored/selected note bodies.
                for note in draw_step.into_iter().flatten() {
                    if note.pitch < pitch_low
                        || u16::from(note.pitch) >= u16::from(pitch_low) + u16::from(pitch_rows)
                    {
                        continue;
                    }
                    let first = i128::from(note.start.raw()).max(beat_start);
                    let last = i128::from(note.start.raw())
                        .saturating_add(i128::from(note.duration.raw()))
                        .min(beat_start.saturating_add(i128::from(raw_span)));
                    if first >= last {
                        continue;
                    }
                    let left = ((first - beat_start) as f64 / raw_span as f64) as f32 * width;
                    let right = ((last - beat_start) as f64 / raw_span as f64) as f32 * width;
                    let row = pitch_low.saturating_add(pitch_rows - 1) - note.pitch;
                    let y = (f32::from(row) + 0.5) * height / f32::from(pitch_rows);
                    quad(
                        left,
                        y - 1.0,
                        right - left,
                        2.0,
                        theme::color(theme::PaletteRole::Primary),
                    );
                }

                // Draw the insertion position last so sustained notes cannot hide it.
                if let Some(marker) = draw_marker.map(i128::from)
                    && marker >= beat_start
                    && marker < beat_start.saturating_add(i128::from(raw_span))
                {
                    let x =
                        (marker.saturating_sub(beat_start) as f64 / raw_span as f64) as f32 * width;
                    quad(x, 0.0, 1.0, height, theme::color(theme::PaletteRole::Text));
                    let cap_left = (x - 3.0).max(0.0);
                    quad(
                        cap_left,
                        0.0,
                        7.0_f32.min(width - cap_left),
                        3.0,
                        theme::color(theme::PaletteRole::Primary),
                    );
                }

                if let Some(playhead) = draw_playhead.map(i128::from)
                    && playhead >= beat_start
                    && playhead < beat_start.saturating_add(i128::from(raw_span))
                {
                    let x = ((playhead - beat_start) as f64 / raw_span as f64) as f32 * width;
                    quad(
                        x,
                        0.0,
                        1.0,
                        height,
                        theme::color(theme::PaletteRole::Secondary),
                    );
                }

                let rendered_context = super::midi_stroke::StrokeContext {
                    generation,
                    revision,
                    clip: clip_id,
                    clip_length: clip_len.raw(),
                    // The checked playback minimum is captured on gesture
                    // admission, never prepared or scanned during painting.
                    minimum_gate: 1,
                    grid,
                    effective_grid: grid,
                    snap_modifier: false,
                    pitch_modifier: false,
                    view_start: beat_start,
                    view_span: beat_span.raw(),
                    pitch_low,
                    pitch_rows,
                    bounds: super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    pitch_locked: draw_pitch_locked,
                    draw_mode,
                };
                entity.read(cx).renew_midi_draw_pointer(
                    rendered_context,
                    entity.entity_id(),
                    window,
                );
                let Some(authoring_length) = rendered_context.authoring_length() else {
                    return;
                };
                let authoring_length = BeatTime::from_raw(authoring_length);
                let wheel_entity = entity.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                    if !phase.capture() || !bounds.contains(&event.position) {
                        return;
                    }
                    let Some(nav) = super::midi_wheel::resolve(
                        event.modifiers,
                        event.delta,
                        event.physical_delta(),
                    ) else {
                        return;
                    };
                    cx.stop_propagation();
                    wheel_entity.update(cx, |view, cx| {
                        let canvas = gesture_bounds_from(bounds);
                        let changed = match nav {
                            super::midi_wheel::WheelNav::Scroll { pitch, time } => {
                                view.scroll_piano_wheel(canvas, pitch, time)
                            }
                            super::midi_wheel::WheelNav::ZoomTime(delta) => {
                                view.zoom_piano_wheel_at(canvas, f32::from(event.position.x), delta)
                            }
                            super::midi_wheel::WheelNav::ZoomPitch(delta) => view
                                .zoom_piano_pitch_wheel_at(
                                    canvas,
                                    f32::from(event.position.y),
                                    delta,
                                ),
                        };
                        if changed {
                            cx.notify();
                        }
                    });
                });
                let mouse_down_entity = entity.clone();
                let event_scene = std::sync::Arc::clone(&canvas_scene_events);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || !matches!(event.button, MouseButton::Left | MouseButton::Middle)
                        || !bounds.contains(&event.position)
                    {
                        return;
                    }
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let x = f32::from(event.position.x - bounds.origin.x);
                    let y = f32::from(event.position.y - bounds.origin.y);
                    let effective_grid = midi_drag::effective_grid(grid, event.modifiers.platform);
                    let gesture_bounds = [
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        width,
                        height,
                    ];
                    mouse_down_entity.update(cx, |view, view_cx| {
                        if view.midi_ruler.cancel() {
                            view_cx.notify();
                        }
                        if view.project_generation == generation
                            && view.state.as_ref().is_some_and(|s| s.revision == revision)
                            && view.detail_clip_id() == Some(clip_id)
                            && (event.button == MouseButton::Middle
                                || (view.midi_pan.enabled && !view.time_selection_key_held))
                        {
                            view.midi_focus.focus(window);
                            if view.begin_piano_pan(
                                rendered_context.bounds,
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.button,
                                window,
                                view_cx,
                            ) {
                                view_cx.notify();
                            }
                            return;
                        }
                        if event.button != MouseButton::Left {
                            return;
                        }
                        if view.project_generation != generation
                            || view.state.as_ref().is_none_or(|s| s.revision != revision)
                            || view.detail_clip_id() != Some(clip_id)
                            || view.edit_pending()
                        {
                            return;
                        }
                        if view.time_selection_key_held {
                            view.cancel_all_midi_marquees();
                            view.midi_focus.focus(window);
                            if view.begin_midi_grid_time_selection(
                                rendered_context.bounds,
                                f32::from(event.position.x),
                                event.modifiers.platform,
                                window,
                                view_cx,
                            ) {
                                view_cx.stop_propagation();
                                view_cx.notify();
                            }
                            return;
                        }
                        view.cancel_all_midi_marquees();
                        view.midi_focus.focus(window);
                        let mut hit_cycle = view.midi_hit_cycle;
                        let hit = event_scene
                            .hit(x, y, width, height, &mut hit_cycle)
                            .and_then(|id| event_scene.note(id));
                        if view.midi_draw_mode {
                            view.clear_midi_time_selection();
                            if event.click_count != 1 {
                                return;
                            }
                            view.midi_hit_cycle = hit_cycle;
                            if view.begin_midi_draw(
                                rendered_context,
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                hit.map(|note| note.id),
                                StrokeModifiers::from_platform(event.modifiers),
                                window,
                                view_cx,
                            ) {
                                view_cx.stop_propagation();
                                view_cx.notify();
                            }
                            return;
                        }
                        if let Some(note) = hit {
                            view.clear_midi_time_selection();
                            view.midi_time_selection.cancel();
                            view.midi_hit_cycle = hit_cycle;
                            if event.click_count >= 2 {
                                view.midi_note_gesture = None;
                                view.midi_group_gesture = None;
                                view.midi_selection.set_single(note.id);
                                view.enqueue(Request::Edit(Command::RemoveMidiNote(note.id)));
                                view_cx.notify();
                                return;
                            }
                            if event.modifiers.shift {
                                let changed = view.midi_selection.toggle(note.id).unwrap_or(false);
                                view.audition_midi_selection();
                                if changed {
                                    view.midi_note_gesture = None;
                                    view.midi_group_gesture = None;
                                }
                                view_cx.notify();
                                return;
                            }
                            let was_selected = view.midi_selection.contains(note.id);
                            if !was_selected {
                                view.midi_selection.set_single(note.id);
                                view.midi_note_gesture = None;
                                view.midi_group_gesture = None;
                            }
                            view.audition_midi_selection();
                            let Some((left, right)) = super::midi_scene::pixel_bounds(note, width)
                            else {
                                return;
                            };
                            let note_width = right - left;
                            let kind = midi_drag::selected_hit_kind(
                                x,
                                left,
                                note_width,
                                note.leading_edge_visible,
                                note.trailing_edge_visible,
                                was_selected,
                            );
                            if view.midi_selection.len() > 1 || event.modifiers.alt {
                                let Some(project_clip) = view
                                    .state
                                    .as_ref()
                                    .and_then(|state| state.project.midi_source(clip_id))
                                else {
                                    return;
                                };
                                let mut originals = Vec::new();
                                if originals
                                    .try_reserve_exact(view.midi_selection.len())
                                    .is_err()
                                {
                                    view.error =
                                        Some("could not reserve selected midi notes".into());
                                    return;
                                }
                                for selected_note in project_clip.notes() {
                                    if !view.midi_selection.contains(selected_note.id()) {
                                        continue;
                                    }
                                    originals.push(super::midi_group_drag::GroupNote {
                                        id: selected_note.id(),
                                        active: selected_note.active(),
                                        start: selected_note.start().raw(),
                                        duration: selected_note.duration().raw(),
                                        pitch: selected_note.pitch(),
                                        channel: selected_note.channel(),
                                        velocity: selected_note.velocity(),
                                    });
                                }
                                if originals.len() != view.midi_selection.len() {
                                    view.midi_group_gesture = None;
                                    return;
                                }
                                view.midi_note_gesture = None;
                                let gesture = super::midi_group_drag::GroupGesture::begin(
                                    super::midi_group_drag::GroupStart {
                                        copy: event.modifiers.alt,
                                        kind,
                                        generation,
                                        revision,
                                        clip: clip_id,
                                        selection_epoch: view.midi_selection.epoch(),
                                        originals,
                                        pointer: [
                                            f32::from(event.position.x),
                                            f32::from(event.position.y),
                                        ],
                                        bounds: gesture_bounds,
                                        beat_start,
                                        beat_span: beat_span.raw(),
                                        pitch_low,
                                        pitch_rows,
                                        clip_length: clip_len.raw(),
                                        grid,
                                    },
                                );
                                if let Some(gesture) = gesture
                                    && view.capture_midi_note_pointer(
                                        rendered_context,
                                        note.id,
                                        gesture.kind(),
                                        gesture.is_copy(),
                                        window,
                                        view_cx,
                                    )
                                {
                                    view.midi_group_gesture = Some(gesture);
                                    view_cx.stop_propagation();
                                }
                                view_cx.notify();
                                return;
                            }
                            let gesture = Some(midi_drag::GestureStart {
                                generation,
                                revision,
                                clip: clip_id,
                                note: note.id,
                                active: note.active,
                                original: NotePreview {
                                    start: note.start,
                                    duration: note.duration,
                                    pitch: note.pitch,
                                },
                                channel: note.channel,
                                velocity: note.velocity,
                                kind,
                                pointer: [f32::from(event.position.x), f32::from(event.position.y)],
                                bounds: gesture_bounds,
                                beat_start,
                                beat_span: beat_span.raw(),
                                clip_length: clip_len.raw(),
                                pitch_low,
                                pitch_rows,
                                grid,
                            })
                            .and_then(midi_drag::begin);
                            if let Some(gesture) = gesture
                                && view.capture_midi_note_pointer(
                                    rendered_context,
                                    note.id,
                                    kind,
                                    false,
                                    window,
                                    view_cx,
                                )
                            {
                                view.midi_note_gesture = Some(gesture);
                                view_cx.stop_propagation();
                            }
                            view_cx.notify();
                            return;
                        }
                        view.midi_note_gesture = None;
                        view.midi_group_gesture = None;
                        view.midi_hit_cycle = None;
                        if !view.midi_draw_mode && event.click_count < 2 {
                            let Some(local_beat) = beat_at_x(
                                x,
                                width,
                                beat_start,
                                beat_span,
                                authoring_length,
                                effective_grid,
                            ) else {
                                return;
                            };
                            view.cancel_midi_audition();
                            view.clear_midi_time_selection();
                            view.midi_time_selection.cancel();
                            view.midi_marker = Some(local_beat.raw());
                            if let Some(marquee) = view
                                .midi_marquee_context(rendered_context.bounds)
                                .and_then(|context| {
                                    super::midi_selection::Marquee::begin(
                                        context,
                                        [x, y],
                                        event.modifiers.shift,
                                        &view.midi_selection,
                                    )
                                })
                                && view.capture_midi_marquee_pointer(
                                    &marquee,
                                    super::midi_marquee::Surface::Piano(std::sync::Arc::clone(
                                        &event_scene,
                                    )),
                                    window,
                                    view_cx,
                                )
                            {
                                view.midi_marquee = Some(marquee);
                                view_cx.stop_propagation();
                            }
                            view_cx.notify();
                            return;
                        }
                        if view.midi_draw_mode && event.click_count > 1 {
                            return;
                        }
                        let Some(local_beat) = beat_at_x(
                            x,
                            width,
                            beat_start,
                            beat_span,
                            authoring_length,
                            effective_grid,
                        ) else {
                            return;
                        };
                        let Some(pitch) = pitch_at_y(y, height, pitch_low, pitch_rows) else {
                            return;
                        };
                        let local = local_beat.raw();
                        let duration = midi_drag::default_gate_raw(
                            authoring_length.raw().saturating_sub(local),
                            effective_grid,
                        );
                        if local >= authoring_length.raw() || duration == 0 {
                            return;
                        }
                        view.clear_midi_time_selection();
                        view.midi_time_selection.cancel();
                        view.cancel_all_midi_marquees();
                        if view.begin_midi_creation(
                            rendered_context,
                            f32::from(event.position.x),
                            hex_project::MidiNoteInsert {
                                active: true,
                                start: local_beat,
                                duration: BeatTime::from_raw(duration),
                                pitch,
                                channel: 0,
                                velocity: 100,
                            },
                            StrokeModifiers::from_platform(event.modifiers),
                            window,
                            view_cx,
                        ) {
                            view_cx.stop_propagation();
                        }
                        view_cx.notify();
                    });
                });
                let mouse_move_entity = entity.clone();
                let hover_scene = std::sync::Arc::clone(&canvas_scene_events);
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    // GPUI's dragging() recognizes only the left button.
                    if matches!(
                        event.pressed_button,
                        Some(MouseButton::Left | MouseButton::Middle)
                    ) || !phase.capture()
                    {
                        return;
                    }
                    mouse_move_entity.update(cx, |view, view_cx| {
                        let target = if hover_allowed
                            && hitbox.is_hovered(window)
                            && view.project_generation == generation
                            && view.state.as_ref().is_some_and(|s| s.revision == revision)
                            && view.detail_clip_id() == Some(clip_id)
                            && !view.edit_pending()
                        {
                            hover_scene.hover(
                                f32::from(event.position.x - bounds.origin.x),
                                f32::from(event.position.y - bounds.origin.y),
                                width,
                                height,
                                view.midi_hit_cycle,
                                view.midi_selection.ids(),
                            )
                        } else {
                            None
                        };
                        if view.midi_hover != target {
                            view.midi_hover = target;
                            view_cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()]);
        let mut panel = div()
            .h(px((self.clip_detail_body_height
                - self.midi_velocity.visible_height())
            .max(64.0)))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
        let source_button = |field,
                             label: &'static str,
                             minus_tooltip: &'static str,
                             plus_tooltip: &'static str| {
            let (value, displayed) = match field {
                super::midi_source_controls::SourceField::ArrangementLength => {
                    (source_values.arrangement_length, "arr")
                }
                super::midi_source_controls::SourceField::SourceLength => {
                    (source_values.source_length, "src")
                }
                super::midi_source_controls::SourceField::SourceStart => {
                    (source_values.source_start, "start")
                }
                super::midi_source_controls::SourceField::LoopStart => {
                    (source_values.loop_start, "scope a")
                }
                super::midi_source_controls::SourceField::LoopEnd => {
                    (source_values.loop_end, "scope b")
                }
                super::midi_source_controls::SourceField::Looping => {
                    (u64::from(source_values.looping), "loop")
                }
            };
            let value_text = if field == super::midi_source_controls::SourceField::Looping {
                if source_values.looping {
                    "on".to_owned()
                } else {
                    "off".to_owned()
                }
            } else {
                format!("{}b", source_beat_label(value))
            };
            let mut group = div().flex().items_center().gap(px(2.0)).child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(format!("{displayed} {value_text}"))),
            );
            if field == super::midi_source_controls::SourceField::Looping {
                group = group.children([self
                    .midi_button_with_id(
                        field.plus_id(),
                        label,
                        plus_tooltip,
                        cx.listener(move |view, _, _, cx| {
                            view.edit_midi_source(clip_id, revision, generation, field, 1);
                            cx.notify();
                        }),
                    )
                    .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))]);
            } else {
                let minus_field = field;
                let plus_field = field;
                group = group
                    .children([self.midi_button_with_id(
                        field.minus_id(),
                        "−",
                        minus_tooltip,
                        cx.listener(move |view, _, _, cx| {
                            view.edit_midi_source(clip_id, revision, generation, minus_field, -1);
                            cx.notify();
                        }),
                    )])
                    .children([self.midi_button_with_id(
                        field.plus_id(),
                        "+",
                        plus_tooltip,
                        cx.listener(move |view, _, _, cx| {
                            view.edit_midi_source(clip_id, revision, generation, plus_field, 1);
                            cx.notify();
                        }),
                    )]);
            }
            group
        };
        let mut source_column = div()
            .id("midi-source-controls")
            .w(px(190.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .p(px(4.0))
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .debug_selector(|| "midi-source-controls".into());
        let source_row = |control| {
            div()
                .h(px(28.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(control)
        };
        source_column = source_column.children(arrangement.map(|_| {
            source_row(source_button(
                super::midi_source_controls::SourceField::ArrangementLength,
                "−",
                "shorten arrangement length by one midi grid step",
                "lengthen arrangement length by one midi grid step",
            ))
        }));
        source_column = source_column.children([source_row(source_button(
                    super::midi_source_controls::SourceField::SourceLength,
                    "−",
                    "shorten source extent by one midi grid step · authored notes and scope markers are retained",
                    "extend source extent by one midi grid step",
                ))])
                .children([source_row(source_button(
                    super::midi_source_controls::SourceField::SourceStart,
                    "−",
                    "move source start earlier by one midi grid step",
                    "move source start later by one midi grid step",
                ))])
                .children([source_row(source_button(
                    super::midi_source_controls::SourceField::LoopStart,
                    "−",
                    "move retained scope start earlier by one midi grid step",
                    "move retained scope start later by one midi grid step",
                ))])
                .children([source_row(source_button(
                    super::midi_source_controls::SourceField::LoopEnd,
                    "−",
                    "move retained scope end earlier by one midi grid step",
                    "move retained scope end later by one midi grid step",
                ))])
                .children([source_row(source_button(
                    super::midi_source_controls::SourceField::Looping,
                    if source_values.looping { "↻" } else { "○" },
                    "toggle source looping · disabled loop markers are retained",
                    "toggle source looping · disabled loop markers are retained",
                ))]);
        let toolbar = div()
            .h(px(28.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .child(theme::text("piano roll"))
            .children([self.midi_button(
                grid.label(),
                if cfg!(target_os = "macos") {
                    "cycle midi grid: 1/4, 1/8, 1/16, 1/32, off · hold cmd to invert snapping · note ends drag free, hold cmd to snap"
                } else {
                    "cycle midi grid: 1/4, 1/8, 1/16, 1/32, off · hold control to invert snapping · note ends drag free, hold control to snap"
                },
                cx.listener(|v, _, _, cx| {
                    v.cycle_midi_grid();
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                if self.midi_draw_mode { "//" } else { "/" },
                if self.midi_draw_mode {
                    "draw mode on · click or drag to draw · press b for normal mode"
                } else {
                    "normal mode · double-click and drag to add/resize · double-click note to remove · b for draw mode"
                },
                cx.listener(|v, _, _, cx| {
                    v.midi_draw_mode = !v.midi_draw_mode;
                    v.cancel_midi_note_gesture();
                    v.midi_group_gesture = None;
                    cx.notify();
                }),
            ).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))])
            .children([self.midi_button(
                if self.midi_draw_pitch_locked {
                    "l"
                } else {
                    "m"
                },
                if self.midi_draw_pitch_locked {
                    "pitch-locked draw · hold option for melodic draw"
                } else {
                    "melodic draw · hold option for pitch-locked draw"
                },
                cx.listener(|v, _, _, cx| {
                    v.midi_draw_pitch_locked = !v.midi_draw_pitch_locked;
                    v.cancel_midi_note_gesture();
                    cx.notify();
                }),
            ).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))])
            .children([self.midi_audition_button(cx)])
            .children([self.midi_button(
                if self.midi_step.enabled { "st" } else { "s" },
                "toggle step entry · enable computer keyboard with m · hold notes and press right · no notes inserts a rest",
                cx.listener(|v, _, window, cx| {
                    v.midi_step.enabled = !v.midi_step.enabled;
                    v.midi_step.cancel_pending();
                    v.cancel_midi_note_gesture();
                    v.cancel_computer_keyboard();
                    v.midi_focus.focus(window);
                    cx.notify();
                }),
            ).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))])
            .children([self.midi_button(
                if self.midi_pan.enabled { "↔+" } else { "↔" },
                "toggle pan mode · drag to move time and pitch · middle-drag always pans",
                cx.listener(|v, _, window, cx| {
                    v.cancel_midi_note_gesture();
                    v.midi_pan.enabled = !v.midi_pan.enabled;
                    v.midi_hover = None;
                    v.midi_focus.focus(window);
                    cx.notify();
                }),
            ).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))])
            .children([self.midi_button(
                "←",
                "pan piano roll left",
                cx.listener(|v, _, _, cx| {
                    v.pan_piano(false);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "→",
                "pan piano roll right",
                cx.listener(|v, _, _, cx| {
                    v.pan_piano(true);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "−",
                "zoom piano roll out",
                cx.listener(|v, _, _, cx| {
                    v.zoom_piano(false);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "+",
                "zoom piano roll in",
                cx.listener(|v, _, _, cx| {
                    v.zoom_piano(true);
                    cx.notify();
                }),
            )])
            .child(self.zoom_fit_button(super::zoom_fit::Target::Midi, false, cx))
            .child(self.zoom_fit_button(super::zoom_fit::Target::Midi, true, cx))
            .children([self.midi_button(
                "o−",
                "lower piano roll pitch range",
                cx.listener(|v, _, _, cx| {
                    v.cancel_midi_note_gesture();
                    v.piano_pitch_low = v.piano_pitch_low.saturating_sub(12).min(128 - v.piano_pitch_rows);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "o+",
                "raise piano roll pitch range",
                cx.listener(|v, _, _, cx| {
                    v.cancel_midi_note_gesture();
                    v.piano_pitch_low = v.piano_pitch_low.saturating_add(12).min(128 - v.piano_pitch_rows);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "♭",
                if group_selected {
                    "selected notes down one semitone"
                } else {
                    "selected note down one semitone"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Pitch(-1),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Pitch(-1));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "♯",
                if group_selected {
                    "selected notes up one semitone"
                } else {
                    "selected note up one semitone"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Pitch(1),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Pitch(1));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "−",
                if group_selected {
                    "shorten selected notes one grid"
                } else {
                    "shorten selected note one grid"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Duration(-1),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Duration(-1));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "+",
                if group_selected {
                    "lengthen selected notes one grid"
                } else {
                    "lengthen selected note one grid"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Duration(1),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Duration(1));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "v−",
                if group_selected {
                    "selected notes velocity down 8"
                } else {
                    "selected note velocity down 8"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Velocity(-8),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Velocity(-8));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "v+",
                if group_selected {
                    "selected notes velocity up 8"
                } else {
                    "selected note velocity up 8"
                },
                cx.listener(move |v, _, _, cx| {
                    if group_selected {
                        v.adjust_selected_group(
                            super::midi_group_drag::GroupParameter::Velocity(8),
                            clip_id,
                            revision,
                            generation,
                            selection_epoch,
                        );
                    } else if let Some(n) =
                        selected.filter(|n| v.midi_selection.primary() == Some(*n))
                    {
                        v.update_note_field(n, revision, generation, NoteField::Velocity(8));
                    }
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "×",
                "delete selected note · backspace",
                cx.listener(|v, _, _, cx| {
                    v.remove_selected_note();
                    cx.notify();
                }),
            )])
            .child(div().flex_1())
            .child(
                div()
                    .max_w(px(240.0))
                    .min_w_0()
                    .flex_shrink_0()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(status)),
            );
        let roll_area = div()
            .flex_1()
            .min_h_0()
            .flex()
            .children([self.piano_keys(pitch_low, cx)])
            .children([roll.children(self.time_selection_cursor_layer())]);
        let source_ruler = self.midi_source_ruler(cx);
        panel = panel.child(source_column).child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(toolbar)
                .children([source_ruler])
                .child(roll_area),
        );
        let velocity = self.velocity_lane(cx);
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(panel)
                .children(velocity),
        )
    }

    pub(super) fn pan_piano(&mut self, right: bool) {
        self.cancel_midi_note_gesture();
        if self.midi_clip().is_none() {
            return;
        }
        let span = self
            .piano_beat_span
            .raw()
            .max((BeatTime::ONE.raw() / 16).max(1));
        let current = self.piano_beat_start;
        self.piano_beat_start = if right {
            super::midi_scene::clamp_start(current.saturating_add(i128::from(span / 2)), span)
        } else {
            super::midi_scene::clamp_start(current.saturating_sub(i128::from(span / 2)), span)
        };
    }

    pub(super) fn cycle_midi_grid(&mut self) {
        self.midi_snap_grid = self.midi_snap_grid.next();
        self.cancel_midi_note_gesture();
    }

    pub(super) fn zoom_piano_pitch_wheel_at(
        &mut self,
        bounds: [f32; 4],
        y: f32,
        delta: f32,
    ) -> bool {
        if self.context_menu.is_some()
            || self.midi_clip().is_none()
            || !delta.is_finite()
            || delta == 0.0
            || !y.is_finite()
            || !bounds.iter().all(|value| value.is_finite())
            || bounds[2] <= 0.0
            || bounds[3] <= 0.0
        {
            return false;
        }
        let fraction = (y - bounds[1]) / bounds[3];
        if !(0.0..=1.0).contains(&fraction) {
            return false;
        }
        self.piano_pitch_wheel_accum = (self.piano_pitch_wheel_accum + delta).clamp(-320.0, 320.0);
        let steps = (self.piano_pitch_wheel_accum / 80.0).trunc() as i8;
        if steps == 0 {
            return false;
        }
        self.piano_pitch_wheel_accum -= f32::from(steps) * 80.0;
        let Some(next) = super::midi_scene::wheel_pitch_zoom(
            self.piano_pitch_low,
            self.piano_pitch_rows,
            fraction,
            steps,
        ) else {
            return false;
        };
        if next == (self.piano_pitch_low, self.piano_pitch_rows) {
            return false;
        }
        self.cancel_midi_note_gesture();
        (self.piano_pitch_low, self.piano_pitch_rows) = next;
        true
    }

    pub(super) fn zoom_piano_wheel_at(&mut self, bounds: [f32; 4], x: f32, delta: f32) -> bool {
        if self.context_menu.is_some() || !delta.is_finite() || delta == 0.0 {
            return false;
        }
        let Some(length) = self
            .midi_clip()
            .map(|source| source.source_length().raw())
            .filter(|length| *length > 0)
        else {
            return false;
        };
        let Some(anchor) = midi_drag::signed_beat_at_point(
            x,
            bounds,
            self.piano_beat_start,
            self.piano_beat_span.raw(),
        ) else {
            return false;
        };
        // Match Arrange's bounded wheel accumulation; keep its state independent.
        self.piano_zoom_wheel_accum = (self.piano_zoom_wheel_accum + delta).clamp(-320.0, 320.0);
        let steps = (self.piano_zoom_wheel_accum / 80.0).trunc() as i8;
        if steps == 0 {
            return false;
        }
        self.piano_zoom_wheel_accum -= f32::from(steps) * 80.0;
        let current = (self.piano_beat_start, self.piano_beat_span.raw());
        let mut next = current;
        for _ in 0..steps.unsigned_abs() {
            next = super::midi_scene::wheel_zoom(next.0, next.1, length, anchor, steps < 0);
        }
        if next == current {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.piano_beat_start = next.0;
        self.piano_beat_span = BeatTime::from_raw(next.1);
        true
    }

    pub(super) fn zoom_piano(&mut self, inward: bool) {
        self.cancel_midi_note_gesture();
        let clip_length = self
            .midi_clip()
            .map_or(BeatTime::ONE.raw(), |clip| clip.source_length().raw());
        let current = self.piano_beat_span.raw();
        self.piano_beat_span = BeatTime::from_raw(super::midi_scene::zoom_span(
            current,
            clip_length,
            BeatTime::ONE.raw(),
            inward,
        ));
        self.piano_beat_start =
            super::midi_scene::clamp_start(self.piano_beat_start, self.piano_beat_span.raw());
    }

    /// Modifier events belong to the focused editor dispatch path. A listener
    /// inside the canvas is below the roll's focus node and misses these events.
    pub(super) fn refresh_piano_note_modifiers(
        &mut self,
        modifiers: gpui::Modifiers,
        window: &mut gpui::Window,
    ) -> bool {
        let pointer = window.mouse_position();
        let x = f32::from(pointer.x);
        let y = f32::from(pointer.y);
        let mut changed = self.update_midi_copy_modifier(modifiers.alt);
        if let Some(gesture) = self.midi_group_gesture.as_mut() {
            changed |= gesture.update([x, y], modifiers.platform);
        }
        if let Some(gesture) = self.midi_note_gesture.as_mut() {
            changed |= midi_drag::update_with_modifier(gesture, x, y, modifiers.platform);
        }
        if let Some(bounds) = self
            .midi_creation
            .as_ref()
            .map(super::midi_creation::Creation::bounds)
        {
            changed |=
                self.update_midi_creation(bounds, x, StrokeModifiers::from_platform(modifiers));
        }
        let modifiers_match = self
            .midi_stroke
            .as_ref()
            .is_none_or(|stroke| stroke.modifiers_match(modifiers.platform, modifiers.alt));
        changed | (!modifiers_match && self.cancel_midi_stroke())
    }

    fn update_midi_copy_modifier(&mut self, copy: bool) -> bool {
        if copy && let Some(note) = self.midi_note_gesture {
            if note.source_prefix > 0 {
                self.midi_note_gesture = None;
                self.error = Some("copying a note before source zero is not available".into());
                return true;
            }
            match super::midi_group_drag::GroupGesture::from_note_copy(
                note,
                self.midi_selection.epoch(),
            ) {
                Ok(Some(group)) => {
                    self.midi_note_gesture = None;
                    self.midi_group_gesture = Some(group);
                    return true;
                }
                Ok(None) => {}
                Err(error) => {
                    // Refuse the requested copy instead of releasing a move.
                    self.midi_note_gesture = None;
                    self.error = Some(error.into());
                    return true;
                }
            }
        }
        if copy
            && self
                .midi_group_gesture
                .as_ref()
                .is_some_and(|gesture| gesture.source_prefix() > 0)
        {
            self.midi_group_gesture = None;
            self.error = Some("copying notes before source zero is not available".into());
            return true;
        }
        self.midi_group_gesture
            .as_mut()
            .is_some_and(|gesture| gesture.set_copy(copy))
    }

    pub(super) fn cancel_midi_note_gesture(&mut self) -> bool {
        let ruler = self.midi_ruler.cancel() | self.midi_time_selection.cancel();
        if ruler {
            self.sync_held_play_start_audition();
        }
        let pan = self.midi_pan.gesture.take().is_some();
        let marquee = self.cancel_all_midi_marquees();
        let velocity = self.midi_velocity.cancel_value_state();
        let pending_selection = self.pending_created_note_selection.take().is_some();
        let pitch_zoom = self.piano_pitch_zoom.take().is_some();
        let creation = self.midi_creation.take().is_some();
        let note = self.midi_note_gesture.take().is_some();
        let group = self.midi_group_gesture.take().is_some();
        let stroke = self.cancel_midi_stroke();
        ruler
            || pan
            || velocity
            || pending_selection
            || pitch_zoom
            || creation
            || note
            || group
            || stroke
            || marquee
    }

    fn piano_keys(&self, low: u8, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.piano_pitch_rows;
        let held = self.midi_live_pitches();
        let entity = cx.entity();
        let input = self.piano_pitch_input(cx);
        let pointer_layer = input.pointer_layer();
        let mut keys = input
            .handle(
                "midi-piano-keys".into(),
                hex_widgets::SplitterHandleConfig::thin(
                    hex_widgets::SplitterAxis::Vertical,
                    theme::color(theme::PaletteRole::Surface),
                ),
            )
            .relative()
            .w(px(38.))
            .h_full()
            .flex_shrink_0();
        for row in 0..rows {
            let pitch = low.saturating_add(rows - 1).saturating_sub(row);
            let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
            let is_c = pitch % 12 == 0;
            let label = if is_c {
                format!("c{} {}", i16::from(pitch) / 12 - 2, pitch)
            } else {
                pitch.to_string()
            };
            keys = keys.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(f32::from(row) / f32::from(rows)))
                    .h(relative(1.0 / f32::from(rows)))
                    .flex()
                    .items_center()
                    .pl(px(2.0))
                    .text_size(px(7.0))
                    .debug_selector(move || format!("midi-piano-key-{pitch}"))
                    .text_color(rgb(if held[usize::from(pitch)] {
                        theme::color(theme::PaletteRole::Background)
                    } else if black {
                        theme::color(theme::PaletteRole::Muted)
                    } else {
                        theme::color(theme::PaletteRole::Text)
                    }))
                    .bg(rgb(if held[usize::from(pitch)] {
                        theme::color(theme::PaletteRole::Primary)
                    } else if black {
                        theme::color(theme::PaletteRole::Background)
                    } else {
                        theme::color(theme::PaletteRole::Raised)
                    }))
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .child(theme::text(label)),
            );
        }
        keys.children([pointer_layer]).child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let wheel_entity = entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                        if !phase.capture() || !bounds.contains(&event.position) {
                            return;
                        }
                        // The keyboard owns only the pitch axis: Alt zooms it and a
                        // plain wheel scrolls it; time pan and Cmd zoom are ignored.
                        let (zoom, delta) = match super::midi_wheel::resolve(
                            event.modifiers,
                            event.delta,
                            event.physical_delta(),
                        ) {
                            Some(super::midi_wheel::WheelNav::ZoomPitch(delta)) => (true, delta),
                            Some(super::midi_wheel::WheelNav::Scroll { pitch, .. })
                                if pitch != 0.0 =>
                            {
                                (false, pitch)
                            }
                            _ => return,
                        };
                        cx.stop_propagation();
                        wheel_entity.update(cx, |view, cx| {
                            let canvas = gesture_bounds_from(bounds);
                            let changed = if zoom {
                                view.zoom_piano_pitch_wheel_at(
                                    canvas,
                                    f32::from(event.position.y),
                                    delta,
                                )
                            } else {
                                view.scroll_piano_wheel(canvas, delta, 0.0)
                            };
                            if changed {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_full(),
        )
    }

    fn midi_audition_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        const HINT: &str =
            "toggle note audition · selection plays up to 17 notes · live midi stays connected";
        let mut style = theme::action_style();
        if self.midi_audition.enabled {
            let control = theme::control_style();
            style.background = control.accent;
            style.text = control.accent_text;
            style.hover_background = control.accent;
            style.hover_text = Some(control.accent_text);
        }
        let ink = style.text;
        hex_widgets::action_surface(style)
            .id(HINT)
            .debug_selector(|| HINT.into())
            .w(px(25.0))
            .h(px(22.0))
            .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
            .children([gpui::svg()
                .path("icons/midi/audition.svg")
                .size(px(14.0))
                .text_color(rgb(ink))
                .debug_selector(|| "midi-audition-speaker".into())])
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: HINT.into(),
                })
                .into()
            })
            .on_click(cx.listener(|v, _, window, cx| {
                v.midi_audition.enabled = !v.midi_audition.enabled;
                v.cancel_midi_audition();
                v.midi_focus.focus(window);
                cx.notify();
            }))
    }

    pub(super) fn midi_button(
        &self,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        self.midi_button_with_id(tooltip, label, tooltip, click)
    }

    pub(super) fn midi_button_with_id(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        let button = hex_widgets::action(theme::text(label), theme::action_style())
            .id(id)
            .w(px(25.0))
            .h(px(22.0))
            .text_xs()
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: tooltip.into(),
                })
                .into()
            })
            .on_click(click);
        #[cfg(feature = "ui-test-support")]
        let button = button.debug_selector(move || tooltip.into());
        button
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_creation_ui_tests.rs"]
pub(super) mod creation_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_zoom_ui_tests.rs"]
mod zoom_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_note_overwrite_ui_tests.rs"]
mod overwrite_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_hover_ui_tests.rs"]
mod hover_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_marquee_ui_tests.rs"]
mod marquee_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_cursor_ui_tests.rs"]
mod cursor_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_step_ui_tests.rs"]
mod step_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_audition_ui_tests.rs"]
mod audition_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_created_selection_ui_tests.rs"]
mod created_selection_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pan_ui_tests.rs"]
mod pan_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_resize_ui_tests.rs"]
mod resize_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_note_copy_ui_tests.rs"]
mod copy_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_input_refinement_ui_tests.rs"]
mod input_refinement_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pending_drag_ui_tests.rs"]
mod pending_drag_tests;
