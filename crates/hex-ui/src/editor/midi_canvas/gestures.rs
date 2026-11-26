//! Piano-roll pan, zoom, grid cycling and modifier gesture handling.
use super::*;

impl EditorView {
    pub(in crate::editor) fn pan_piano(&mut self, right: bool) {
        self.cancel_midi_note_gesture();
        if self.midi_clip().is_none() {
            return;
        }
        let span = self
            .midi_editor
            .piano_beat_span
            .raw()
            .max((BeatTime::ONE.raw() / 16).max(1));
        let current = self.midi_editor.piano_beat_start;
        self.midi_editor.piano_beat_start = if right {
            crate::editor::midi_scene::clamp_start(
                current.saturating_add(i128::from(span / 2)),
                span,
            )
        } else {
            crate::editor::midi_scene::clamp_start(
                current.saturating_sub(i128::from(span / 2)),
                span,
            )
        };
    }

    pub(in crate::editor) fn cycle_midi_grid(&mut self) {
        self.midi_editor.snap_grid = self.midi_editor.snap_grid.next();
        self.cancel_midi_note_gesture();
    }

    pub(in crate::editor) fn zoom_piano_pitch_wheel_at(
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
        let fraction = (f64::from(y) - f64::from(bounds[1])) / f64::from(bounds[3]);
        let Some(next) = self
            .midi_editor
            .piano_pitch
            .zoom_at(bounds[3], fraction, delta)
        else {
            return false;
        };
        if next == self.midi_editor.piano_pitch {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_editor.piano_pitch = next;
        self.midi_editor.piano_pitch_note_fit = false;
        true
    }

    pub(in crate::editor) fn zoom_piano_wheel_at(
        &mut self,
        bounds: [f32; 4],
        x: f32,
        delta: f32,
    ) -> bool {
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
            self.midi_editor.piano_beat_start,
            self.midi_editor.piano_beat_span.raw(),
        ) else {
            return false;
        };
        // Match Arrange's bounded wheel accumulation; keep its state independent.
        self.midi_editor.piano_zoom_wheel_accum =
            (self.midi_editor.piano_zoom_wheel_accum + delta).clamp(-320.0, 320.0);
        let steps = (self.midi_editor.piano_zoom_wheel_accum / 80.0).trunc() as i8;
        if steps == 0 {
            return false;
        }
        self.midi_editor.piano_zoom_wheel_accum -= f32::from(steps) * 80.0;
        let current = (
            self.midi_editor.piano_beat_start,
            self.midi_editor.piano_beat_span.raw(),
        );
        let mut next = current;
        for _ in 0..steps.unsigned_abs() {
            next = crate::editor::midi_scene::wheel_zoom(next.0, next.1, length, anchor, steps < 0);
        }
        if next == current {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_editor.piano_beat_start = next.0;
        self.midi_editor.piano_beat_span = BeatTime::from_raw(next.1);
        true
    }

    pub(in crate::editor) fn zoom_piano(&mut self, inward: bool) {
        self.cancel_midi_note_gesture();
        let clip_length = self
            .midi_clip()
            .map_or(BeatTime::ONE.raw(), |clip| clip.source_length().raw());
        let current = self.midi_editor.piano_beat_span.raw();
        self.midi_editor.piano_beat_span = BeatTime::from_raw(
            crate::editor::midi_scene::zoom_span(current, clip_length, BeatTime::ONE.raw(), inward),
        );
        self.midi_editor.piano_beat_start = crate::editor::midi_scene::clamp_start(
            self.midi_editor.piano_beat_start,
            self.midi_editor.piano_beat_span.raw(),
        );
    }

    /// Modifier events belong to the focused editor dispatch path. A listener
    /// inside the canvas is below the roll's focus node and misses these events.
    pub(in crate::editor) fn refresh_piano_note_modifiers(
        &mut self,
        modifiers: gpui::Modifiers,
        window: &mut gpui::Window,
    ) -> bool {
        let pointer = window.mouse_position();
        let x = f32::from(pointer.x);
        let y = f32::from(pointer.y);
        let mut changed = self.update_midi_copy_modifier(modifiers.alt);
        if let Some(gesture) = self.midi_editor.group_gesture.as_mut() {
            changed |= gesture.update([x, y], modifiers.platform);
        }
        if let Some(gesture) = self.midi_editor.note_gesture.as_mut() {
            changed |= midi_drag::update_with_modifier(gesture, x, y, modifiers.platform);
        }
        if let Some(bounds) = self
            .midi_editor
            .creation
            .as_ref()
            .map(crate::editor::midi_creation::Creation::bounds)
        {
            changed |=
                self.update_midi_creation(bounds, x, StrokeModifiers::from_platform(modifiers));
        }
        let modifiers_match = self
            .midi_editor
            .stroke
            .as_ref()
            .is_none_or(|stroke| stroke.modifiers_match(modifiers.platform, modifiers.alt));
        changed | (!modifiers_match && self.cancel_midi_stroke())
    }

    pub(super) fn update_midi_copy_modifier(&mut self, copy: bool) -> bool {
        if copy && let Some(note) = self.midi_editor.note_gesture {
            if note.source_prefix > 0 {
                self.midi_editor.note_gesture = None;
                self.error = Some("copying a note before source zero is not available".into());
                return true;
            }
            match crate::editor::midi_group_drag::GroupGesture::from_note_copy(
                note,
                self.midi_editor.selection.epoch(),
            ) {
                Ok(Some(group)) => {
                    self.midi_editor.note_gesture = None;
                    self.midi_editor.group_gesture = Some(group);
                    return true;
                }
                Ok(None) => {}
                Err(error) => {
                    // Refuse the requested copy instead of releasing a move.
                    self.midi_editor.note_gesture = None;
                    self.error = Some(error.into());
                    return true;
                }
            }
        }
        if copy
            && self
                .midi_editor
                .group_gesture
                .as_ref()
                .is_some_and(|gesture| gesture.source_prefix() > 0)
        {
            self.midi_editor.group_gesture = None;
            self.error = Some("copying notes before source zero is not available".into());
            return true;
        }
        self.midi_editor
            .group_gesture
            .as_mut()
            .is_some_and(|gesture| gesture.set_copy(copy))
    }

    pub(in crate::editor) fn cancel_midi_note_gesture(&mut self) -> bool {
        let ruler = self.midi_editor.ruler.cancel() | self.midi_editor.time_selection.cancel();
        if ruler {
            self.sync_held_play_start_audition();
        }
        let pan = self.midi_editor.pan.gesture.take().is_some();
        let marquee = self.cancel_all_midi_marquees();
        let velocity = self.midi_editor.velocity.cancel_value_state();
        let pending_selection = self
            .midi_editor
            .pending_created_note_selection
            .take()
            .is_some();
        let pitch_zoom = self.midi_editor.piano_pitch_zoom.take().is_some();
        let creation = self.midi_editor.creation.take().is_some();
        let note = self.midi_editor.note_gesture.take().is_some();
        let group = self.midi_editor.group_gesture.take().is_some();
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
}
