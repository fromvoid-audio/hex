//! Normal-mode held note creation. Preview stays view-owned until one canonical
//! note edit on release, including source growth if needed. Existing canvas
//! identity/geometry and grid rules bound the gesture; the dragged end is free
//! unless cmd/control snaps it (`midi_drag::note_end_grid`). Released display
//! transfers to midi_pending_drag's exact ordinary receipt; it never retains held
//! input.
//! The Window left-button lease owns the second double-click press only.
use super::{
    EditorView, midi_drag,
    midi_draw::{self, StrokeModifiers},
    midi_stroke::{CanvasBounds, StrokeContext},
};
use gpui::{Context, MouseButton, MouseMoveEvent, MouseUpEvent, Window, point, px, size};
use hex_project::{BeatTime, ClipId, Command, MidiNoteInsert};

/// Immutable input identity; only duration/dragged change during a held creation.
#[derive(Clone, Copy, PartialEq)]
struct CaptureKey {
    context: StrokeContext,
    down_x: f32,
    start: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

pub(super) struct Creation {
    context: StrokeContext,
    down_x: f32,
    dragged: bool,
    minimum_gate: u64,
    pub(super) preview: MidiNoteInsert,
}

impl Creation {
    fn key(&self) -> CaptureKey {
        CaptureKey {
            context: self.context,
            down_x: self.down_x,
            start: self.preview.start,
            pitch: self.preview.pitch,
            channel: self.preview.channel,
            velocity: self.preview.velocity,
        }
    }

    pub(super) fn bounds(&self) -> [f32; 4] {
        let b = self.context.bounds;
        [b.x, b.y, b.width, b.height]
    }
}

impl EditorView {
    pub(super) fn midi_minimum_creation_gate(&self) -> Option<u64> {
        let state = self.state.as_ref()?;
        if let Some(creation) = self.midi_creation.as_ref()
            && creation.context.generation == self.project_generation
            && creation.context.revision == state.revision
        {
            return Some(creation.minimum_gate);
        }
        if let Some(gate) = self
            .midi_stroke
            .as_ref()
            .and_then(|stroke| stroke.minimum_gate_for(self.project_generation, state.revision))
        {
            return Some(gate);
        }
        let project = &state.project;
        let fastest = project
            .tempo_map()
            .points()
            .iter()
            .map(|point| point.tempo())
            .max_by_key(|tempo| tempo.micro_bpm())?;
        // Two frames leave rounding headroom at every authored tempo boundary.
        hex_project::TempoMap::constant(project.sample_rate(), fastest)
            .ok()?
            .frame_to_beat(2)
            .ok()
            .map(|beat| beat.raw().max(1))
    }

    pub(super) fn begin_midi_creation(
        &mut self,
        context: StrokeContext,
        x: f32,
        mut note: MidiNoteInsert,
        modifiers: StrokeModifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(length) = context.authoring_length() else {
            return false;
        };
        let current = self.current_stroke_context(context.bounds, modifiers);
        if !context.bounds.is_valid()
            || !x.is_finite()
            || note.start.raw() >= length
            || note.duration.raw() == 0
            || note
                .start
                .raw()
                .checked_add(note.duration.raw())
                .is_none_or(|end| end > length)
            || self.edit_pending()
            || current.is_none_or(|current| {
                current.draw_mode || !midi_draw::same_rendered_frame(current, context)
            })
        {
            return false;
        }
        let Some(minimum_gate) = self.midi_minimum_creation_gate() else {
            return false;
        };
        let Some(duration) = midi_drag::creation_gate(
            length - note.start.raw(),
            midi_drag::effective_grid(context.grid, modifiers.snap),
            context.view_span,
            context.bounds.width,
            minimum_gate,
        ) else {
            return false;
        };
        note.duration = BeatTime::from_raw(duration);
        let creation = Creation {
            context,
            down_x: x,
            dragged: false,
            minimum_gate,
            preview: note,
        };
        if !self.capture_midi_creation(creation.key(), window, cx) {
            return false;
        }
        self.midi_creation = Some(creation);
        true
    }

    fn owns_midi_creation_capture(&self, key: CaptureKey) -> bool {
        self.midi_creation.as_ref().is_some_and(|g| g.key() == key)
    }
    fn capture_midi_creation(
        &self,
        key: CaptureKey,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            gpui::CursorStyle::ResizeRight,
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
                    if !view.owns_midi_creation_capture(key) {
                        return;
                    }
                    if !view.clip_view_open
                        || view.device_panel_active
                        || view.context_menu.is_some()
                        || !view.midi_focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        view.midi_creation = None;
                        cx.notify();
                        return;
                    }
                    let modifiers = moved
                        .map(|e| e.modifiers)
                        .or_else(|| released.map(|e| e.modifiers))
                        .unwrap_or_default();
                    let b = key.context.bounds;
                    let bounds = [b.x, b.y, b.width, b.height];
                    if released.is_some() {
                        let inside = gpui::Bounds::new(
                            point(px(b.x), px(b.y)),
                            size(px(b.width), px(b.height)),
                        )
                        .contains(&position);
                        view.finish_midi_creation(
                            bounds,
                            f32::from(position.x),
                            inside,
                            StrokeModifiers::from_platform(modifiers),
                        );
                        cx.notify();
                    } else if view.update_midi_creation(
                        bounds,
                        f32::from(position.x),
                        StrokeModifiers::from_platform(modifiers),
                    ) {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.owns_midi_creation_capture(key) {
                        view.midi_creation = None;
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// Actual canvas/source scalars witness only held Creation. The released
    /// displayed_midi_creation query includes accepted receipt geometry and must
    /// never renew input. No project or tempo lookup occurs while painting.
    pub(super) fn renew_midi_creation_capture(
        &self,
        bounds: CanvasBounds,
        clip: ClipId,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        if self.clip_view_open
            && !self.device_panel_active
            && self.context_menu.is_none()
            && self.midi_focus.is_focused(window)
            && !self.edit_pending()
            && let Some(g) = self.midi_creation.as_ref()
            && g.context.bounds == bounds
            && g.context.clip == clip
            && g.context.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.context.revision)
            && self.piano_beat_start == g.context.view_start
            && self.piano_beat_span.raw() == g.context.view_span
            && self.piano_pitch_low == g.context.pitch_low
            && self.piano_pitch_rows == g.context.pitch_rows
            && self.midi_snap_grid == g.context.grid
            && self.midi_draw_mode == g.context.draw_mode
            && self.midi_draw_pitch_locked == g.context.pitch_locked
        {
            window.renew_pointer_capture(owner, gpui::CursorStyle::ResizeRight);
        }
    }

    pub(super) fn update_midi_creation(
        &mut self,
        bounds: [f32; 4],
        x: f32,
        modifiers: StrokeModifiers,
    ) -> bool {
        let Some(current) =
            self.current_stroke_context(midi_draw::canvas_bounds(bounds), modifiers)
        else {
            return self.midi_creation.take().is_some();
        };
        let Some(length) = current.authoring_length() else {
            return self.midi_creation.take().is_some();
        };
        let pending = self.edit_pending();
        let Some(creation) = self.midi_creation.as_mut() else {
            return false;
        };
        if pending
            || current.draw_mode
            || !midi_draw::same_rendered_frame(current, creation.context)
            || !x.is_finite()
        {
            self.midi_creation = None;
            return true;
        }
        creation.dragged |= if current.effective_grid == midi_drag::MidiGrid::Off {
            x != creation.down_x
        } else {
            (x - creation.down_x).abs() >= 3.0
        };
        if !creation.dragged {
            let Some(gate) = midi_drag::creation_gate(
                length - creation.preview.start.raw(),
                current.effective_grid,
                current.view_span,
                current.bounds.width,
                creation.minimum_gate,
            ) else {
                self.midi_creation = None;
                return true;
            };
            let duration = BeatTime::from_raw(gate);
            let changed = creation.preview.duration != duration;
            creation.preview.duration = duration;
            return changed;
        }
        let Some(raw) = midi_drag::beat_at_point(x, bounds, current.view_start, current.view_span)
        else {
            self.midi_creation = None;
            return true;
        };
        let start = creation.preview.start.raw();
        let remaining = length.saturating_sub(start);
        let end_grid = midi_drag::note_end_grid(current.grid, current.snap_modifier);
        let minimum = end_grid
            .minimum_gate()
            .max(creation.minimum_gate)
            .min(remaining);
        let end = midi_drag::snap_raw(raw, end_grid).clamp(start.saturating_add(minimum), length);
        let duration = BeatTime::from_raw(end - start);
        let changed = creation.preview.duration != duration;
        creation.preview.duration = duration;
        changed
    }

    pub(super) fn finish_midi_creation(
        &mut self,
        bounds: [f32; 4],
        x: f32,
        inside: bool,
        modifiers: StrokeModifiers,
    ) {
        if !inside {
            self.midi_creation = None;
            return;
        }
        self.update_midi_creation(bounds, x, modifiers);
        if let Some(creation) = self.midi_creation.take() {
            let note = creation.preview;
            let grows =
                note.start.raw().saturating_add(note.duration.raw()) > creation.context.clip_length;
            let command = if grows {
                Command::EditMidiNotesWithSourceExtent {
                    clip: creation.context.clip,
                    remove: Vec::new(),
                    insert: vec![note],
                }
            } else {
                Command::AddMidiNote {
                    clip: creation.context.clip,
                    start: note.start,
                    duration: note.duration,
                    pitch: note.pitch,
                    channel: note.channel,
                    velocity: note.velocity,
                }
            };
            self.submit_midi_drag_preview(
                creation.context.clip,
                super::midi_pending_drag::Frozen::Creation(note),
                command,
            );
        }
    }
}
