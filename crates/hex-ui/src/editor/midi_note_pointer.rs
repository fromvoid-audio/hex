//! Held single/group note input. Pure drag models retain geometry/copy rules;
//! midi_pending_drag retains accepted display. Alt promotion is one input scope,
//! so callback identity follows source/frame/anchor/kind, not the mutable variant.
use super::EditorView;
use crate::editor::{
    midi_drag::{self, GestureContext, GestureKind, HitKind},
    midi_group_drag::GroupContext,
    midi_stroke::{CanvasBounds, StrokeContext},
};
use gpui::{Context, CursorStyle, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent, Window};
use hex_project::{BeatTime, ClipId, Command, NoteId};

#[derive(Clone, Copy)]
struct Key {
    frame: StrokeContext,
    note: NoteId,
    kind: HitKind,
}
fn rectangle(b: CanvasBounds) -> [f32; 4] {
    [b.x, b.y, b.width, b.height]
}
fn kind(g: GestureKind) -> HitKind {
    match g {
        GestureKind::Move { .. } => HitKind::Move,
        GestureKind::ResizeLeading { .. } => HitKind::ResizeLeading,
        GestureKind::ResizeTrailing { .. } => HitKind::ResizeTrailing,
    }
}
fn cursor(kind: HitKind, copy: bool) -> CursorStyle {
    match kind {
        HitKind::Move if copy => CursorStyle::DragCopy,
        HitKind::Move => CursorStyle::ClosedHand,
        HitKind::ResizeLeading => CursorStyle::ResizeLeft,
        HitKind::ResizeTrailing => CursorStyle::ResizeRight,
    }
}
impl EditorView {
    // The pressed note came from the captured source. Canonical note edits
    // install a new revision, so later pointer events need only check identity.
    fn note_pointer_single_context(&self, c: GroupContext) -> GestureContext {
        GestureContext {
            generation: c.generation,
            revision: c.revision,
            selected_clip: c.clip,
            selected_note: self.midi_selection.primary(),
            pending: c.pending,
            beat_start: c.beat_start,
            beat_span: c.beat_span,
            pitch_low: c.pitch_low,
            pitch_rows: c.pitch_rows,
            grid: c.grid,
            bounds: c.bounds,
        }
    }
    fn note_pointer_context(&self, bounds: [f32; 4], clip: Option<ClipId>) -> Option<GroupContext> {
        Some(GroupContext {
            generation: self.project_generation,
            revision: self.state.as_ref()?.revision,
            clip,
            selection_epoch: self.midi_selection.epoch(),
            beat_start: self.piano_beat_start,
            beat_span: self.piano_beat_span.raw(),
            pitch_low: self.piano_pitch_low,
            pitch_rows: self.piano_pitch_rows,
            grid: self.midi_snap_grid,
            bounds,
            pending: self.edit_pending(),
        })
    }
    /// Compare only the immutable held snapshot. Current-state validation belongs
    /// to update/release; cancellation must still find a now-stale held draft.
    fn owns_note_pointer(&self, key: Key) -> bool {
        let f = key.frame;
        if let Some(g) = &self.midi_group_gesture {
            let c = g.captured_context();
            return c.generation == f.generation
                && c.revision == f.revision
                && c.clip == Some(f.clip)
                && c.beat_start == f.view_start
                && c.beat_span == f.view_span
                && c.pitch_low == f.pitch_low
                && c.pitch_rows == f.pitch_rows
                && c.grid == f.grid
                && c.bounds == rectangle(f.bounds)
                && g.kind() == key.kind
                && g.preview(key.note).is_some();
        }
        self.midi_note_gesture.as_ref().is_some_and(|g| {
            g.note == key.note
                && kind(g.kind) == key.kind
                && midi_drag::is_current(
                    g,
                    GestureContext {
                        generation: f.generation,
                        revision: f.revision,
                        selected_clip: Some(f.clip),
                        selected_note: Some(key.note),
                        pending: false,
                        beat_start: f.view_start,
                        beat_span: f.view_span,
                        pitch_low: f.pitch_low,
                        pitch_rows: f.pitch_rows,
                        grid: f.grid,
                        bounds: rectangle(f.bounds),
                    },
                )
        })
    }
    fn cancel_note_pointer(&mut self, key: Key) -> bool {
        if !self.owns_note_pointer(key) {
            return false;
        }
        self.midi_note_gesture = None;
        self.midi_group_gesture = None;
        self.cancel_midi_audition();
        true
    }
    pub(super) fn capture_midi_note_pointer(
        &self,
        frame: StrokeContext,
        note: NoteId,
        hit: HitKind,
        copy: bool,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let key = Key {
            frame,
            note,
            kind: hit,
        };
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            cursor(hit, copy),
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
                    if !view.owns_note_pointer(key) {
                        return;
                    }
                    if !view.clip_view_open
                        || view.device_panel_active
                        || view.midi_draw_mode
                        || view.context_menu.is_some()
                        || !view.midi_focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        if view.cancel_note_pointer(key) {
                            cx.notify();
                        }
                        return;
                    }
                    let modifiers = moved
                        .map(|e| e.modifiers)
                        .or_else(|| released.map(|e| e.modifiers))
                        .unwrap_or_default();
                    let point = [f32::from(position.x), f32::from(position.y)];
                    if released.is_some() {
                        view.finish_midi_note_pointer(key, point, modifiers);
                        cx.notify();
                    } else if view.update_midi_note_pointer(key, point, modifiers) {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.cancel_note_pointer(key) {
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    fn update_midi_note_pointer(
        &mut self,
        key: Key,
        point: [f32; 2],
        modifiers: Modifiers,
    ) -> bool {
        let mut changed = self.update_midi_copy_modifier(modifiers.alt);
        let Some(context) =
            self.note_pointer_context(rectangle(key.frame.bounds), self.detail_clip_id())
        else {
            return self.cancel_note_pointer(key) | changed;
        };
        if let Some(g) = &mut self.midi_group_gesture {
            if !g.is_current(context) {
                self.midi_group_gesture = None;
                return true;
            }
            changed |= g.update(point, modifiers.platform);
            return changed;
        }
        let current = self
            .midi_note_gesture
            .as_ref()
            .is_some_and(|g| midi_drag::is_current(g, self.note_pointer_single_context(context)));
        if !current {
            return self.midi_note_gesture.take().is_some() | changed;
        }
        if let Some(g) = &mut self.midi_note_gesture {
            changed |= midi_drag::update_with_modifier(g, point[0], point[1], modifiers.platform);
        }
        changed
    }
    fn finish_midi_note_pointer(&mut self, key: Key, point: [f32; 2], modifiers: Modifiers) {
        self.update_midi_copy_modifier(modifiers.alt);
        let Some(context) =
            self.note_pointer_context(rectangle(key.frame.bounds), self.detail_clip_id())
        else {
            self.cancel_note_pointer(key);
            return;
        };
        let clip = key.frame.clip;
        if let Some(mut g) = self.midi_group_gesture.take() {
            let _ = g.update(point, modifiers.platform);
            if g.is_current(context)
                && !g.is_noop()
                && midi_drag::point_is_inside(g.bounds(), point[0], point[1])
            {
                if g.is_copy() {
                    match g.copied_notes() {
                        Ok(insert) => self.submit_midi_drag_preview(
                            clip,
                            crate::editor::midi_pending_drag::Frozen::Group(g),
                            Command::EditMidiNotesWithSourceExtent {
                                clip,
                                remove: Vec::new(),
                                insert,
                            },
                        ),
                        Err(error) => self.error = Some(error.into()),
                    }
                } else {
                    match g.updates() {
                        Ok(notes) => {
                            let command = if g.source_prefix() > 0 {
                                Command::UpdateMidiNotesWithSourcePrefix {
                                    clip,
                                    prefix: BeatTime::from_raw(g.source_prefix()),
                                    notes,
                                }
                            } else if notes.iter().any(|note| {
                                note.start
                                    .raw()
                                    .checked_add(note.duration.raw())
                                    .is_none_or(|end| end > g.source_length())
                            }) {
                                Command::UpdateMidiNotesWithSourceExtent { clip, notes }
                            } else {
                                Command::UpdateMidiNotes { clip, notes }
                            };
                            self.submit_midi_drag_preview(
                                clip,
                                crate::editor::midi_pending_drag::Frozen::Group(g),
                                command,
                            );
                        }
                        Err(error) => self.error = Some(error.into()),
                    }
                }
            }
            return;
        }
        let Some(mut g) = self.midi_note_gesture.take() else {
            return;
        };
        if !midi_drag::is_current(&g, self.note_pointer_single_context(context))
            || !midi_drag::release_is_inside(&g, point[0], point[1])
        {
            return;
        }
        midi_drag::update_with_modifier(&mut g, point[0], point[1], modifiers.platform);
        if !midi_drag::is_noop(&g) {
            let command = if g.source_prefix > 0 {
                Command::UpdateMidiNotesWithSourcePrefix {
                    clip,
                    prefix: BeatTime::from_raw(g.source_prefix),
                    notes: vec![hex_project::MidiNoteUpdate {
                        note: g.note,
                        start: BeatTime::from_raw(g.preview.start),
                        duration: BeatTime::from_raw(g.preview.duration),
                        pitch: g.preview.pitch,
                        channel: g.channel,
                        velocity: g.velocity,
                    }],
                }
            } else if g
                .preview
                .start
                .checked_add(g.preview.duration)
                .is_none_or(|end| end > g.clip_length)
            {
                Command::UpdateMidiNotesWithSourceExtent {
                    clip,
                    notes: vec![hex_project::MidiNoteUpdate {
                        note: g.note,
                        start: BeatTime::from_raw(g.preview.start),
                        duration: BeatTime::from_raw(g.preview.duration),
                        pitch: g.preview.pitch,
                        channel: g.channel,
                        velocity: g.velocity,
                    }],
                }
            } else {
                Command::UpdateMidiNote {
                    note: g.note,
                    start: BeatTime::from_raw(g.preview.start),
                    duration: BeatTime::from_raw(g.preview.duration),
                    pitch: g.preview.pitch,
                    channel: g.channel,
                    velocity: g.velocity,
                }
            };
            self.submit_midi_drag_preview(
                clip,
                crate::editor::midi_pending_drag::Frozen::Single(g),
                command,
            );
        }
    }
    /// Only held models witness input. Their accepted Frozen equivalents never
    /// renew. Paint uses the actual rendered clip and immutable revision, avoiding
    /// note/project scans; full source membership stays in the existing updater.
    pub(super) fn renew_midi_note_pointer(
        &self,
        bounds: CanvasBounds,
        clip: ClipId,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        if !self.clip_view_open
            || self.device_panel_active
            || self.midi_draw_mode
            || self.context_menu.is_some()
            || !self.midi_focus.is_focused(window)
        {
            return;
        }
        let Some(mut c) = self.note_pointer_context(rectangle(bounds), Some(clip)) else {
            return;
        };
        // A held click remains owned while unrelated preparation is pending;
        // update/release still apply their original pending-command validation.
        c.pending = false;
        let held = if let Some(g) = &self.midi_group_gesture {
            g.is_current(c).then(|| cursor(g.kind(), g.is_copy()))
        } else {
            self.midi_note_gesture
                .as_ref()
                .filter(|g| midi_drag::is_current(g, self.note_pointer_single_context(c)))
                .map(|g| cursor(kind(g.kind), false))
        };
        if let Some(style) = held {
            window.renew_pointer_capture(owner, style);
        }
    }
}
