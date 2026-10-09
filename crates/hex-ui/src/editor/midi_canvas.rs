//! Compact piano-roll Canvas rendering and pointer interaction.
use super::midi_drag::{self, NotePreview};
use super::midi_draw::StrokeModifiers;
use super::{EditorView, midi::NoteField};
use crate::theme;
use gpui::{
    Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window,
    canvas, div, fill, point, prelude::*, px, relative, rgb, size,
};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

fn beat_at_x(
    x: f32,
    width: f32,
    view_start: BeatTime,
    beat_span: BeatTime,
    clip_length: BeatTime,
    grid: midi_drag::MidiGrid,
) -> Option<BeatTime> {
    if !x.is_finite()
        || !width.is_finite()
        || width <= 0.0
        || x < 0.0
        || x >= width
        || beat_span.raw() == 0
    {
        return None;
    }
    let offset = (f64::from(x / width) * beat_span.raw() as f64) as u64;
    let raw = view_start.raw().saturating_add(offset);
    let start = if let Some(step) = grid.step_raw() {
        let last_grid = clip_length.raw().saturating_sub(1) / step * step;
        midi_drag::snap_raw(raw, grid).min(last_grid)
    } else {
        raw.min(clip_length.raw().saturating_sub(1))
    };
    let beat = BeatTime::from_raw(start);
    (beat.raw() < clip_length.raw()).then_some(beat)
}

fn beat_label(raw: u64) -> String {
    let value = raw as f64 / BeatTime::ONE.raw() as f64;
    let mut text = format!("{value:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn source_beat_label(raw: u64) -> String {
    let label = beat_label(raw);
    if label.len() <= 7 {
        label
    } else {
        format!(
            "{:.2e}",
            raw as f64 / hex_project::BeatTime::ONE.raw() as f64
        )
    }
}

fn pitch_at_y(y: f32, height: f32, low: u8) -> Option<u8> {
    if !y.is_finite() || !height.is_finite() || height <= 0.0 || y < 0.0 || y >= height || low > 104
    {
        return None;
    }
    Some(
        low.saturating_add(23)
            .saturating_sub(((y / height * 24.0).floor() as u8).min(23)),
    )
}

impl EditorView {
    pub(super) fn midi_editor(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.state.as_ref()?;
        let clip_id = self.selected_clip?;
        let source = state.project.midi_source(clip_id)?;
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
        let clip_len = source.source_length();
        let beat_start = self.piano_beat_start;
        let beat_span = self.piano_beat_span;
        let pitch_low = self.piano_pitch_low;
        let grid = self.midi_snap_grid;
        let notes = source.notes();
        let key = super::midi_scene::SceneKey {
            generation,
            revision,
            clip: clip_id,
            beat_start: beat_start.raw(),
            beat_span: beat_span.raw(),
            pitch_low,
        };
        let scene = self.midi_scene_cache.get_or_build(key, notes).0;
        let selected = if self.midi_selection.len() == 1 {
            self.midi_selection.primary()
        } else {
            None
        };
        let group_selected = self.midi_selection.len() > 1;
        let selected_ids = self.midi_selection.shared_ids();
        let selection_epoch = self.midi_selection.epoch();
        let preview = self.midi_note_gesture.map(|gesture| gesture.preview);
        let selected_note_data = selected.and_then(|id| state.project.midi_note(id));
        let status = if self.midi_selection.len() > 1 {
            format!(
                "{} notes selected · drag to move · backspace to delete · cmd+a select all",
                self.midi_selection.len()
            )
        } else {
            selected_note_data.map_or_else(
                || {
                    format!(
                        "{} beats · click: select time · double-click: add",
                        beat_label(beat_span.raw())
                    )
                },
                |note| {
                    let data = preview.filter(|_| selected == Some(note.id()));
                    let (pitch, start, duration) = data.map_or_else(
                        || (note.pitch(), note.start().raw(), note.duration().raw()),
                        |item| (item.pitch, item.start, item.duration),
                    );
                    format!(
                        "p{pitch} · t{} · g{} · v{}",
                        beat_label(start),
                        beat_label(duration),
                        note.velocity()
                    )
                },
            )
        };

        let canvas_scene = std::sync::Arc::clone(&scene);
        let draw_selected = selected_ids;
        let draw_preview = self
            .midi_note_gesture
            .map(|gesture| (gesture.note, gesture.preview));
        let draw_group = self
            .midi_group_gesture
            .as_ref()
            .map(|gesture| (gesture.shared_originals(), gesture.clone()));
        let draw_stroke = self
            .midi_stroke
            .as_ref()
            .map(|stroke| stroke.shared_preview());
        let draw_marquee = self.midi_marquee;
        let draw_marker = self.midi_marker;
        let draw_pitch_locked = self.midi_draw_pitch_locked;
        let draw_mode = self.midi_draw_mode;
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
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                if width <= 0.0 || height <= 0.0 {
                    return;
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
                for row in 0..=super::midi_scene::ROWS {
                    let y = row as f32 * height / super::midi_scene::ROWS as f32;
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
                let grid_step = grid.step_raw().unwrap_or(BeatTime::ONE.raw()).max(1);
                let first = beat_start.raw() / grid_step;
                let last = beat_start.raw().saturating_add(raw_span) / grid_step;
                let beat_count = last.saturating_sub(first).saturating_add(1);
                let stride = beat_count.saturating_add(255).saturating_div(256).max(1);
                let first_tick = first
                    .saturating_add(stride - 1)
                    .saturating_div(stride)
                    .saturating_mul(stride);
                for index in 0..256_u64 {
                    let beat = first_tick.saturating_add(index.saturating_mul(stride));
                    if beat > last {
                        break;
                    }
                    let raw = beat.saturating_mul(grid_step);
                    let x = (raw.saturating_sub(beat_start.raw()) as f64 / raw_span as f64) as f32
                        * width;
                    quad(
                        x,
                        0.0,
                        1.0,
                        height,
                        if raw % (4 * BeatTime::ONE.raw()) == 0 {
                            theme::color(theme::PaletteRole::Border)
                        } else {
                            theme::color(theme::PaletteRole::Grid)
                        },
                    );
                }
                if let Some(marker) = draw_marker
                    && marker >= beat_start.raw()
                    && marker < beat_start.raw().saturating_add(raw_span)
                {
                    let x = (marker.saturating_sub(beat_start.raw()) as f64 / raw_span as f64)
                        as f32
                        * width;
                    quad(
                        x,
                        0.0,
                        1.0,
                        height,
                        theme::color(theme::PaletteRole::Primary),
                    );
                }
                for note in canvas_scene.notes() {
                    if draw_selected.binary_search(&note.id).is_ok()
                        || draw_stroke
                            .as_ref()
                            .is_some_and(|stroke| stroke.removed.binary_search(&note.id).is_ok())
                        || draw_preview.is_some_and(|(id, _)| id == note.id)
                        || draw_group.as_ref().is_some_and(|(originals, _)| {
                            originals
                                .binary_search_by_key(&note.id, |item| item.id)
                                .is_ok()
                        })
                    {
                        continue;
                    }
                    let Some((x, right)) = super::midi_scene::pixel_bounds(*note, width) else {
                        continue;
                    };
                    let y = note.y * height;
                    let w = right - x;
                    quad(
                        x,
                        y + 1.0,
                        w,
                        (height / 24.0 - 2.0).max(1.0),
                        theme::color(theme::PaletteRole::Secondary),
                    );
                    if w >= 6.0 && note.leading_edge_visible {
                        quad(
                            x,
                            y + 1.0,
                            1.0,
                            (height / 24.0 - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                    if w >= 6.0 && note.trailing_edge_visible {
                        quad(
                            x + w - 1.0,
                            y + 1.0,
                            1.0,
                            (height / 24.0 - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                }
                for id in draw_selected.iter().copied() {
                    if draw_preview.is_some_and(|(preview_id, _)| preview_id == id)
                        || draw_stroke
                            .as_ref()
                            .is_some_and(|stroke| stroke.removed.binary_search(&id).is_ok())
                        || draw_group.as_ref().is_some_and(|(originals, _)| {
                            originals.binary_search_by_key(&id, |item| item.id).is_ok()
                        })
                    {
                        continue;
                    }
                    let Some(note) = canvas_scene.note(id) else {
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
                        (height / 24.0 - 2.0).max(1.0),
                        theme::color(theme::PaletteRole::Primary),
                    );
                    if w >= 6.0 && note.leading_edge_visible {
                        quad(
                            x,
                            y + 1.0,
                            1.0,
                            (height / 24.0 - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                    if w >= 6.0 && note.trailing_edge_visible {
                        quad(
                            x + w - 1.0,
                            y + 1.0,
                            1.0,
                            (height / 24.0 - 2.0).max(1.0),
                            theme::color(theme::PaletteRole::Text),
                        );
                    }
                }

                if let Some(stroke) = &draw_stroke {
                    for note in &stroke.inserted {
                        if let Some(rect) = canvas_scene.insert_geometry(*note) {
                            let x = rect.x * width;
                            let right = (rect.x + rect.width) * width;
                            quad(
                                x,
                                rect.y * height + 1.0,
                                right - x,
                                (height / 24.0 - 2.0).max(1.0),
                                theme::color(theme::PaletteRole::Primary),
                            );
                        }
                    }
                }
                if let Some((id, item)) = draw_preview {
                    if let Some(note) = canvas_scene.preview_geometry(
                        id,
                        item.start,
                        item.duration,
                        item.pitch,
                        0,
                        100,
                    ) {
                        if let Some((x, right)) = super::midi_scene::pixel_bounds(note, width) {
                            quad(
                                x,
                                note.y * height + 1.0,
                                right - x,
                                (height / 24.0 - 2.0).max(1.0),
                                theme::color(theme::PaletteRole::Primary),
                            );
                        }
                    }
                }

                if let Some((originals, gesture)) = &draw_group {
                    for original in originals.iter() {
                        let Some(preview) = gesture.preview(original.id) else {
                            continue;
                        };
                        let Some(note) = canvas_scene.preview_geometry(
                            original.id,
                            preview.start,
                            preview.duration,
                            preview.pitch,
                            preview.channel,
                            preview.velocity,
                        ) else {
                            continue;
                        };
                        if let Some((x, right)) = super::midi_scene::pixel_bounds(note, width) {
                            quad(
                                x,
                                note.y * height + 1.0,
                                right - x,
                                (height / 24.0 - 2.0).max(1.0),
                                theme::color(theme::PaletteRole::Primary),
                            );
                        }
                    }
                }

                if let Some((first, last, _)) = draw_marquee {
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

                let rendered_context = super::midi_stroke::StrokeContext {
                    generation,
                    revision,
                    clip: clip_id,
                    clip_length: clip_len.raw(),
                    grid,
                    effective_grid: grid,
                    snap_modifier: false,
                    pitch_modifier: false,
                    view_start: beat_start.raw(),
                    view_span: beat_span.raw(),
                    pitch_low,
                    bounds: super::midi_stroke::CanvasBounds {
                        x: f32::from(bounds.origin.x),
                        y: f32::from(bounds.origin.y),
                        width,
                        height,
                    },
                    pitch_locked: draw_pitch_locked,
                    draw_mode,
                };
                let mouse_down_entity = entity.clone();
                let event_scene = std::sync::Arc::clone(&canvas_scene_events);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || event.button != MouseButton::Left
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
                        if view.project_generation != generation
                            || view.state.as_ref().is_none_or(|s| s.revision != revision)
                            || view.selected_clip != Some(clip_id)
                            || view.edit_pending()
                        {
                            return;
                        }
                        view.midi_focus.focus(window);
                        let mut hit_cycle = view.midi_hit_cycle;
                        let hit = event_scene
                            .hit(x, y, width, height, &mut hit_cycle)
                            .and_then(|id| event_scene.note(id));
                        if view.midi_draw_mode {
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
                            ) {
                                view_cx.notify();
                            }
                            return;
                        }
                        if let Some(note) = hit {
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
                                if changed {
                                    view.midi_note_gesture = None;
                                    view.midi_group_gesture = None;
                                }
                                view_cx.notify();
                                return;
                            }
                            if !view.midi_selection.contains(note.id) {
                                view.midi_selection.set_single(note.id);
                                view.midi_note_gesture = None;
                                view.midi_group_gesture = None;
                            }
                            let Some((left, right)) = super::midi_scene::pixel_bounds(note, width)
                            else {
                                return;
                            };
                            let note_width = right - left;
                            let kind = midi_drag::hit_kind(
                                x,
                                left,
                                note_width,
                                note.leading_edge_visible,
                                note.trailing_edge_visible,
                            );
                            if view.midi_selection.len() > 1 {
                                if kind != midi_drag::HitKind::Move {
                                    view.midi_note_gesture = None;
                                    view.midi_group_gesture = None;
                                    view_cx.notify();
                                    return;
                                }
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
                                view.midi_group_gesture =
                                    super::midi_group_drag::GroupGesture::begin(
                                        super::midi_group_drag::GroupStart {
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
                                            beat_start: beat_start.raw(),
                                            beat_span: beat_span.raw(),
                                            pitch_low,
                                            clip_length: clip_len.raw(),
                                            grid,
                                        },
                                    );
                                view_cx.notify();
                                return;
                            }
                            view.midi_note_gesture = Some(midi_drag::GestureStart {
                                generation,
                                revision,
                                clip: clip_id,
                                note: note.id,
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
                                beat_start: beat_start.raw(),
                                beat_span: beat_span.raw(),
                                clip_length: clip_len.raw(),
                                pitch_low,
                                grid,
                            })
                            .and_then(midi_drag::begin);
                            view_cx.notify();
                            return;
                        }
                        view.midi_note_gesture = None;
                        view.midi_group_gesture = None;
                        view.midi_hit_cycle = None;
                        if !view.midi_draw_mode && event.click_count < 2 {
                            if !event.modifiers.shift {
                                view.midi_selection.clear();
                            }
                            let Some(local_beat) = beat_at_x(
                                x,
                                width,
                                beat_start,
                                beat_span,
                                clip_len,
                                midi_drag::MidiGrid::Off,
                            ) else {
                                return;
                            };
                            view.midi_marker = Some(local_beat.raw());
                            view.midi_marquee = Some(([x, y], [x, y], event.modifiers.shift));
                            view_cx.notify();
                            return;
                        }
                        if view.midi_draw_mode && event.click_count > 1 {
                            return;
                        }
                        let Some(local_beat) =
                            beat_at_x(x, width, beat_start, beat_span, clip_len, effective_grid)
                        else {
                            return;
                        };
                        let Some(pitch) = pitch_at_y(y, height, pitch_low) else {
                            return;
                        };
                        let local = local_beat.raw();
                        let duration = midi_drag::default_gate_raw(
                            clip_len.raw().saturating_sub(local),
                            effective_grid,
                        );
                        if local >= clip_len.raw() || duration == 0 {
                            return;
                        }
                        view.midi_marquee = None;
                        view.midi_selection.clear();
                        view.enqueue(Request::Edit(Command::AddMidiNote {
                            clip: clip_id,
                            start: local_beat,
                            duration: BeatTime::from_raw(duration),
                            pitch,
                            channel: 0,
                            velocity: 100,
                        }));
                        view_cx.notify();
                    });
                });
                let mouse_move_entity = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() || !event.dragging() {
                        return;
                    }
                    mouse_move_entity.update(cx, |view, view_cx| {
                        if let Some((first, _, additive)) = view.midi_marquee {
                            let last = [
                                f32::from(event.position.x - bounds.origin.x),
                                f32::from(event.position.y - bounds.origin.y),
                            ];
                            view.midi_marquee = Some((first, last, additive));
                            view_cx.notify();
                            return;
                        }
                        let pending = view.edit_pending();
                        if view.midi_stroke.is_some() {
                            if view.update_midi_draw(
                                [
                                    f32::from(bounds.origin.x),
                                    f32::from(bounds.origin.y),
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                ],
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                StrokeModifiers::from_platform(event.modifiers),
                            ) {
                                view_cx.notify();
                            }
                            return;
                        }
                        if let Some(gesture) = &mut view.midi_group_gesture {
                            let current =
                                gesture.is_current(super::midi_group_drag::GroupContext {
                                    generation,
                                    revision,
                                    clip: view.selected_clip,
                                    selection_epoch: view.midi_selection.epoch(),
                                    beat_start: beat_start.raw(),
                                    beat_span: beat_span.raw(),
                                    pitch_low,
                                    grid,
                                    bounds: [
                                        f32::from(bounds.origin.x),
                                        f32::from(bounds.origin.y),
                                        f32::from(bounds.size.width),
                                        f32::from(bounds.size.height),
                                    ],
                                    pending,
                                });
                            if !current {
                                view.midi_group_gesture = None;
                                view_cx.notify();
                                return;
                            }
                            if gesture.update(
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.modifiers.platform,
                            ) {
                                view_cx.notify();
                            }
                            return;
                        }
                        let context = midi_drag::GestureContext {
                            generation,
                            revision,
                            selected_clip: view.selected_clip,
                            selected_note: view.midi_selection.primary(),
                            clip_contains_note: false,
                            pending: false,
                            beat_start: beat_start.raw(),
                            beat_span: beat_span.raw(),
                            pitch_low,
                            grid,
                            bounds: [
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ],
                        };
                        let current = view.midi_note_gesture.as_ref().is_some_and(|gesture| {
                            view.midi_note_gesture_current(gesture, context)
                        });
                        if !current {
                            if view.midi_note_gesture.take().is_some() {
                                view_cx.notify();
                            }
                            return;
                        }
                        if let Some(gesture) = &mut view.midi_note_gesture
                            && midi_drag::update_with_modifier(
                                gesture,
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                                event.modifiers.platform,
                            )
                        {
                            view_cx.notify();
                        }
                    });
                });
                let mouse_up_entity = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if !phase.bubble() || event.button != MouseButton::Left {
                        return;
                    }
                    mouse_up_entity.update(cx, |view, view_cx| {
                        if view.midi_stroke.is_some() {
                            view.finish_midi_draw(
                                [
                                    f32::from(bounds.origin.x),
                                    f32::from(bounds.origin.y),
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                ],
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                bounds.contains(&event.position),
                                StrokeModifiers::from_platform(event.modifiers),
                            );
                            view_cx.notify();
                            return;
                        }
                        if view.project_generation != generation
                            || view
                                .state
                                .as_ref()
                                .is_none_or(|state| state.revision != revision)
                            || view.selected_clip != Some(clip_id)
                            || view.edit_pending()
                        {
                            view.midi_note_gesture = None;
                            view.midi_group_gesture = None;
                            view.midi_marquee = None;
                            view_cx.notify();
                            return;
                        }
                        if let Some((first, last, additive)) = view.midi_marquee.take() {
                            let dx = (first[0] - last[0]).abs();
                            let dy = (first[1] - last[1]).abs();
                            if dx >= 3.0 || dy >= 3.0 {
                                let hits = super::midi_selection::marquee_hits(
                                    canvas_scene_events.notes(),
                                    first,
                                    last,
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                );
                                match hits {
                                    Ok(ids) => {
                                        let result = if additive {
                                            view.midi_selection.add_many(&ids)
                                        } else {
                                            view.midi_selection.select_all(ids)
                                        };
                                        if let Err(error) = result {
                                            view.error = Some(error.into());
                                        }
                                    }
                                    Err(error) => view.error = Some(error.into()),
                                }
                            }
                            view_cx.notify();
                            return;
                        }
                        if let Some(mut gesture) = view.midi_group_gesture.take() {
                            let pending = view.edit_pending();
                            let context = super::midi_group_drag::GroupContext {
                                generation,
                                revision,
                                clip: view.selected_clip,
                                selection_epoch: view.midi_selection.epoch(),
                                beat_start: beat_start.raw(),
                                beat_span: beat_span.raw(),
                                pitch_low,
                                grid,
                                bounds: [
                                    f32::from(bounds.origin.x),
                                    f32::from(bounds.origin.y),
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                ],
                                pending,
                            };
                            let _ = gesture.update(
                                [f32::from(event.position.x), f32::from(event.position.y)],
                                event.modifiers.platform,
                            );
                            if gesture.is_current(context)
                                && !gesture.is_noop()
                                && midi_drag::point_is_inside(
                                    gesture.bounds(),
                                    f32::from(event.position.x),
                                    f32::from(event.position.y),
                                )
                            {
                                match gesture.updates() {
                                    Ok(notes) => {
                                        view.enqueue(Request::Edit(Command::UpdateMidiNotes {
                                            clip: clip_id,
                                            notes,
                                        }));
                                    }
                                    Err(error) => view.error = Some(error.into()),
                                }
                            }
                            view_cx.notify();
                            return;
                        }
                        let Some(mut gesture) = view.midi_note_gesture.take() else {
                            return;
                        };
                        let context = midi_drag::GestureContext {
                            generation,
                            revision,
                            selected_clip: view.selected_clip,
                            selected_note: view.midi_selection.primary(),
                            clip_contains_note: false,
                            pending: false,
                            beat_start: beat_start.raw(),
                            beat_span: beat_span.raw(),
                            pitch_low,
                            grid,
                            bounds: [
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ],
                        };
                        if !view.midi_note_gesture_current(&gesture, context)
                            || !midi_drag::release_is_inside(
                                &gesture,
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            )
                        {
                            view_cx.notify();
                            return;
                        }
                        midi_drag::update_with_modifier(
                            &mut gesture,
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            event.modifiers.platform,
                        );
                        if !midi_drag::is_noop(&gesture) {
                            view.enqueue(Request::Edit(Command::UpdateMidiNote {
                                note: gesture.note,
                                start: BeatTime::from_raw(gesture.preview.start),
                                duration: BeatTime::from_raw(gesture.preview.duration),
                                pitch: gesture.preview.pitch,
                                channel: gesture.channel,
                                velocity: gesture.velocity,
                            }));
                        }
                        view_cx.notify();
                    });
                });
                let modifier_entity = entity.clone();
                window.on_modifiers_changed(move |event, _, cx| {
                    modifier_entity.update(cx, |view, view_cx| {
                        let modifiers_match = view.midi_stroke.as_ref().is_none_or(|stroke| {
                            stroke.modifiers_match(event.modifiers.platform, event.modifiers.alt)
                        });
                        if !modifiers_match && view.cancel_midi_stroke() {
                            view_cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()]);
        let mut panel = div()
            .h(px(super::clip_view::CLIP_DETAIL_BODY_HEIGHT))
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
                    (source_values.loop_start, "loop a")
                }
                super::midi_source_controls::SourceField::LoopEnd => {
                    (source_values.loop_end, "loop b")
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
                group = group.children([self.midi_button_with_id(
                    field.plus_id(),
                    label,
                    plus_tooltip,
                    cx.listener(move |view, _, _, cx| {
                        view.edit_midi_source(clip_id, revision, generation, field, 1);
                        cx.notify();
                    }),
                )]);
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
                    "shorten source extent by one midi grid step · authored notes and loop markers are retained",
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
                    "move retained loop start earlier by one midi grid step",
                    "move retained loop start later by one midi grid step",
                ))])
                .children([source_row(source_button(
                    super::midi_source_controls::SourceField::LoopEnd,
                    "−",
                    "move retained loop end earlier by one midi grid step",
                    "move retained loop end later by one midi grid step",
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
                    "cycle midi grid: 1/4, 1/8, 1/16, 1/32, off · hold cmd to invert snapping"
                } else {
                    "cycle midi grid: 1/4, 1/8, 1/16, 1/32, off · hold control to invert snapping"
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
                    "normal mode · double-click to add or remove · press b for draw mode"
                },
                cx.listener(|v, _, _, cx| {
                    v.midi_draw_mode = !v.midi_draw_mode;
                    v.cancel_midi_note_gesture();
                    v.midi_group_gesture = None;
                    v.midi_marquee = None;
                    cx.notify();
                }),
            )])
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
            )])
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
            .children([self.midi_button(
                "⤢",
                "fit midi clip in piano roll",
                cx.listener(|v, _, _, cx| {
                    v.fit_piano();
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "o−",
                "lower piano roll pitch range",
                cx.listener(|v, _, _, cx| {
                    v.cancel_midi_note_gesture();
                    v.piano_pitch_low = v.piano_pitch_low.saturating_sub(12).min(104);
                    cx.notify();
                }),
            )])
            .children([self.midi_button(
                "o+",
                "raise piano roll pitch range",
                cx.listener(|v, _, _, cx| {
                    v.cancel_midi_note_gesture();
                    v.piano_pitch_low = v.piano_pitch_low.saturating_add(12).min(104);
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
            .children([self.piano_keys(pitch_low)])
            .children([roll]);
        panel = panel.child(source_column).child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(toolbar)
                .child(roll_area),
        );
        Some(panel)
    }

    fn pan_piano(&mut self, right: bool) {
        self.cancel_midi_note_gesture();
        let clip_length = self
            .midi_clip()
            .map_or(0, |clip| clip.source_length().raw());
        let span = self
            .piano_beat_span
            .raw()
            .max((BeatTime::ONE.raw() / 16).max(1));
        let current = self.piano_beat_start.raw();
        self.piano_beat_start = BeatTime::from_raw(if right {
            super::midi_scene::clamp_start(current.saturating_add(span / 2), clip_length, span)
        } else {
            super::midi_scene::clamp_start(current.saturating_sub(span / 2), clip_length, span)
        });
    }

    fn cycle_midi_grid(&mut self) {
        self.midi_snap_grid = self.midi_snap_grid.next();
        self.cancel_midi_note_gesture();
    }

    fn zoom_piano(&mut self, inward: bool) {
        self.cancel_midi_note_gesture();
        let clip_length = self
            .midi_clip()
            .map_or(BeatTime::ONE.raw(), |clip| clip.source_length().raw());
        let current = self
            .piano_beat_span
            .raw()
            .max((BeatTime::ONE.raw() / 16).max(1));
        self.piano_beat_span = BeatTime::from_raw(super::midi_scene::zoom_span(
            current,
            clip_length,
            BeatTime::ONE.raw(),
            inward,
        ));
        self.piano_beat_start = BeatTime::from_raw(super::midi_scene::clamp_start(
            self.piano_beat_start.raw(),
            clip_length,
            self.piano_beat_span.raw(),
        ));
    }

    fn fit_piano(&mut self) {
        self.cancel_midi_note_gesture();
        let length = self
            .midi_clip()
            .map_or(BeatTime::ONE.raw(), |clip| clip.source_length().raw());
        self.piano_beat_span =
            BeatTime::from_raw(super::midi_scene::fit_span(length, BeatTime::ONE.raw()));
        self.piano_beat_start = BeatTime::ZERO;
    }

    pub(super) fn cancel_midi_note_gesture(&mut self) -> bool {
        let note = self.midi_note_gesture.take().is_some();
        let group = self.midi_group_gesture.take().is_some();
        let stroke = self.cancel_midi_stroke();
        let marquee = self.midi_marquee.take().is_some();
        note || group || stroke || marquee
    }

    fn midi_note_gesture_current(
        &self,
        gesture: &midi_drag::NoteGesture,
        mut context: midi_drag::GestureContext,
    ) -> bool {
        let Some(clip) = context.selected_clip else {
            return false;
        };
        context.pending = self.edit_pending();
        let clip_contains_note = self.state.as_ref().is_some_and(|state| {
            state.revision == context.revision
                && state
                    .project
                    .midi_source(clip)
                    .is_some_and(|midi| midi.notes().iter().any(|note| note.id() == gesture.note))
        });
        context.clip_contains_note = clip_contains_note;
        context.generation == self.project_generation && midi_drag::is_current(gesture, context)
    }

    fn piano_keys(&self, low: u8) -> Div {
        let mut keys = div()
            .relative()
            .w(px(38.0))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        for row in 0..24_u8 {
            let pitch = low.saturating_add(23).saturating_sub(row);
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
                    .top(relative(f32::from(row) / 24.0))
                    .h(relative(1.0 / 24.0))
                    .flex()
                    .items_center()
                    .pl(px(2.0))
                    .text_size(px(7.0))
                    .text_color(rgb(if black {
                        theme::color(theme::PaletteRole::Muted)
                    } else {
                        theme::color(theme::PaletteRole::Text)
                    }))
                    .bg(rgb(if black {
                        theme::color(theme::PaletteRole::Background)
                    } else {
                        theme::color(theme::PaletteRole::Raised)
                    }))
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .child(theme::text(label)),
            );
        }
        keys
    }

    fn midi_button(
        &self,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        self.midi_button_with_id(tooltip, label, tooltip, click)
    }

    fn midi_button_with_id(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .w(px(25.0))
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .hover(|s| s.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: tooltip.into(),
                })
                .into()
            })
            .on_click(click)
            .child(theme::text(label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn piano_grid_and_pitch_mapping_stay_in_clip_and_midi_bounds() {
        let clip = BeatTime::from_quarters(4).unwrap();
        assert_eq!(
            beat_at_x(
                25.0,
                100.0,
                BeatTime::ZERO,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            )
            .unwrap(),
            BeatTime::ONE
        );
        assert_eq!(
            beat_at_x(
                100.0,
                100.0,
                BeatTime::ZERO,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            ),
            None
        );
        assert_eq!(
            beat_at_x(
                f32::NAN,
                100.0,
                BeatTime::ZERO,
                BeatTime::from_quarters(4).unwrap(),
                clip,
                midi_drag::MidiGrid::Sixteenth,
            ),
            None
        );
        assert_eq!(pitch_at_y(0.0, 120.0, 48), Some(71));
        for (pitch, low) in [(48_u8, 48_u8), (71, 48), (104, 104), (127, 104)] {
            let row = low + 23 - pitch;
            let y = (f32::from(row) + 0.5) / 24.0 * 120.0;
            assert_eq!(pitch_at_y(y, 120.0, low), Some(pitch));
        }
        assert_eq!(pitch_at_y(120.0, 120.0, 48), None);
        assert_eq!(pitch_at_y(f32::INFINITY, 120.0, 48), None);
        assert_eq!((i16::from(0_u8) - 1).clamp(0, 127), 0);
        assert_eq!((i16::from(127_u8) + 1).clamp(0, 127), 127);
        assert_eq!(4 * BeatTime::ONE.raw(), 4 * 4 * BeatTime::ONE.raw() / 4);
    }

    #[test]
    fn piano_click_time_uses_clip_local_selected_grid_or_exact_off_time() {
        let clip = BeatTime::from_quarters(4).unwrap();
        let span = BeatTime::from_quarters(4).unwrap();
        let x = 31.25;
        for (grid, expected) in [
            (midi_drag::MidiGrid::Quarter, BeatTime::ONE.raw()),
            (
                midi_drag::MidiGrid::Eighth,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 2,
            ),
            (
                midi_drag::MidiGrid::Sixteenth,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
            (
                midi_drag::MidiGrid::ThirtySecond,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
            (
                midi_drag::MidiGrid::Off,
                BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4,
            ),
        ] {
            assert_eq!(
                beat_at_x(x, 100.0, BeatTime::ZERO, span, clip, grid)
                    .unwrap()
                    .raw(),
                expected
            );
        }

        let x = 31.0;
        let free = midi_drag::effective_grid(midi_drag::MidiGrid::Sixteenth, true);
        let snapped_from_free = midi_drag::effective_grid(midi_drag::MidiGrid::Off, true);
        assert_eq!(free, midi_drag::MidiGrid::Off);
        assert_eq!(snapped_from_free, midi_drag::MidiGrid::Sixteenth);
        let free_start = beat_at_x(x, 100.0, BeatTime::ZERO, span, clip, free)
            .unwrap()
            .raw();
        assert!(free_start > BeatTime::ONE.raw() + BeatTime::ONE.raw() * 239 / 1000);
        assert!(free_start < BeatTime::ONE.raw() + BeatTime::ONE.raw() * 241 / 1000);
        assert_ne!(free_start, BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4);
        assert_eq!(
            beat_at_x(x, 100.0, BeatTime::ZERO, span, clip, snapped_from_free)
                .unwrap()
                .raw(),
            BeatTime::ONE.raw() + BeatTime::ONE.raw() / 4
        );
    }

    #[test]
    fn compact_gate_label_retains_precision_for_subquarter_notes() {
        assert_eq!(beat_label(BeatTime::ONE.raw() / 512), "0.002");
        assert_eq!(beat_label(BeatTime::ONE.raw()), "1");
    }

    #[test]
    fn source_control_values_stay_bounded_at_the_beat_domain_limit() {
        assert_eq!(source_beat_label(4 * BeatTime::ONE.raw()), "4");
        assert!(source_beat_label(u64::MAX).len() <= 7);
    }
}
