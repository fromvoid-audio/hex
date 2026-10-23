//! Normal-mode held note creation. Preview stays view-owned until one canonical
//! note edit on release, including source growth if needed. Existing canvas
//! identity/geometry and grid rules bound the gesture.
use super::{
    EditorView, midi_drag,
    midi_draw::{self, StrokeModifiers},
    midi_stroke::StrokeContext,
};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;

pub(super) struct Creation {
    context: StrokeContext,
    down_x: f32,
    dragged: bool,
    minimum_gate: u64,
    pub(super) preview: MidiNoteInsert,
}

impl Creation {
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
    ) {
        let Some(length) = context.authoring_length() else {
            return;
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
            return;
        }
        let Some(minimum_gate) = self.midi_minimum_creation_gate() else {
            return;
        };
        let Some(duration) = midi_drag::creation_gate(
            length - note.start.raw(),
            midi_drag::effective_grid(context.grid, modifiers.snap),
            context.view_span,
            context.bounds.width,
            minimum_gate,
        ) else {
            return;
        };
        note.duration = BeatTime::from_raw(duration);
        self.midi_creation = Some(Creation {
            context,
            down_x: x,
            dragged: false,
            minimum_gate,
            preview: note,
        });
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
        let minimum = current
            .effective_grid
            .minimum_gate()
            .max(creation.minimum_gate)
            .min(remaining);
        let end = midi_drag::snap_raw(raw, current.effective_grid)
            .clamp(start.saturating_add(minimum), length);
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
            self.enqueue(Request::Edit(command));
        }
    }
}
