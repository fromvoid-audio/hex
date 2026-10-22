//! Normal-mode held note creation. Preview stays view-owned until one AddMidiNote
//! on release; existing canvas identity/geometry and grid rules bound the gesture.
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
    pub(super) preview: MidiNoteInsert,
}

impl EditorView {
    pub(super) fn begin_midi_creation(
        &mut self,
        context: StrokeContext,
        x: f32,
        note: MidiNoteInsert,
    ) {
        let current = self.current_stroke_context(context.bounds, StrokeModifiers::default());
        if !context.bounds.is_valid()
            || !x.is_finite()
            || note.start.raw() >= context.clip_length
            || note.duration.raw() == 0
            || note
                .start
                .raw()
                .checked_add(note.duration.raw())
                .is_none_or(|end| end > context.clip_length)
            || self.edit_pending()
            || current.is_none_or(|current| {
                current.draw_mode || !midi_draw::same_rendered_frame(current, context)
            })
        {
            return;
        }
        self.midi_creation = Some(Creation {
            context,
            down_x: x,
            dragged: false,
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
        creation.dragged |= (x - creation.down_x).abs() >= 3.0;
        if !creation.dragged {
            return false;
        }
        let Some(raw) = midi_drag::beat_at_point(x, bounds, current.view_start, current.view_span)
        else {
            self.midi_creation = None;
            return true;
        };
        let start = creation.preview.start.raw();
        let remaining = current.clip_length.saturating_sub(start);
        let minimum = current.effective_grid.minimum_gate().min(remaining);
        let end = midi_drag::snap_raw(raw, current.effective_grid)
            .clamp(start.saturating_add(minimum), current.clip_length);
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
            self.enqueue(Request::Edit(Command::AddMidiNote {
                clip: creation.context.clip,
                start: note.start,
                duration: note.duration,
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            }));
        }
    }
}
