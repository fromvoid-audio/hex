//! Control-side MIDI draw stroke admission and coordinate mapping.
//! Pure resolution and accepted display custody retain their existing owners.
use super::{EditorView, midi_drag};
use super::{midi_stroke, midi_stroke::StrokeContext};
use gpui::{Context, Window};

#[path = "midi_draw_pointer.rs"]
mod pointer;
#[cfg(test)]
use hex_project::BeatTime;
use hex_project::{ClipId, MidiNote, NoteId};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StrokeModifiers {
    pub(super) snap: bool,
    pub(super) pitch: bool,
}

impl StrokeModifiers {
    pub(super) fn from_platform(modifiers: gpui::Modifiers) -> Self {
        Self {
            snap: modifiers.platform,
            pitch: modifiers.alt,
        }
    }
}

impl EditorView {
    pub(super) fn begin_midi_draw(
        &mut self,
        rendered: StrokeContext,
        point: [f32; 2],
        hit: Option<NoteId>,
        modifiers: StrokeModifiers,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> bool {
        let Some(context) = self.current_stroke_context(rendered.bounds, modifiers) else {
            return false;
        };
        if !same_rendered_frame(context, rendered)
            || !self.midi_draw_mode
            || self.edit_pending()
            || self.midi_stroke.is_some()
            || self.project_generation != rendered.generation
        {
            return false;
        }
        let Some(down) = stroke_point(context, point[0], point[1], false) else {
            return false;
        };
        let Some(clip) = self
            .state
            .as_ref()
            .and_then(|s| s.project.midi_source(context.clip))
        else {
            return false;
        };
        if clip.notes().len() > hex_project::MAX_NOTES {
            self.error = Some("midi clip exceeds the draw stroke note limit".into());
            return false;
        }
        let mut baseline = Vec::new();
        if baseline.try_reserve_exact(clip.notes().len()).is_err() {
            self.error = Some("could not reserve midi draw baseline".into());
            return false;
        }
        baseline.extend(clip.notes().iter().map(|note| midi_stroke::BaselineNote {
            id: note.id(),
            start: note.start().raw(),
            duration: note.duration().raw(),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        }));
        match midi_stroke::MidiStroke::begin_with_hit(
            context,
            context.clip_length,
            Arc::from(baseline),
            down,
            hit,
        ) {
            Ok(stroke) => {
                if !self.capture_midi_draw_pointer(&stroke, window, cx) {
                    return false;
                }
                self.midi_note_gesture = None;
                self.midi_group_gesture = None;
                self.cancel_all_midi_marquees();
                self.midi_focus.focus(window);
                self.midi_stroke = Some(stroke);
                true
            }
            Err(error) => {
                self.error = Some(stroke_error(error).into());
                false
            }
        }
    }

    pub(super) fn update_midi_draw(
        &mut self,
        bounds: [f32; 4],
        point: [f32; 2],
        modifiers: StrokeModifiers,
    ) -> bool {
        let Some(context) = self.current_stroke_context(canvas_bounds(bounds), modifiers) else {
            return self.cancel_midi_stroke();
        };
        let Some(point) = stroke_point(context, point[0], point[1], true) else {
            return self.cancel_midi_stroke();
        };
        let pending = self.edit_pending();
        let Some(stroke) = self.midi_stroke.as_mut() else {
            return false;
        };
        match stroke.update(context, pending, point) {
            Ok(changed) => changed,
            Err(error) => {
                if error != midi_stroke::StrokeError::InvalidContext {
                    self.error = Some(stroke_error(error).into());
                }
                self.midi_stroke = None;
                true
            }
        }
    }

    pub(super) fn finish_midi_draw(
        &mut self,
        bounds: [f32; 4],
        point: [f32; 2],
        inside: bool,
        modifiers: StrokeModifiers,
    ) -> bool {
        let Some(mut stroke) = self.midi_stroke.take() else {
            return false;
        };
        if !inside {
            stroke.cancel();
            return true;
        }
        let Some(context) = self.current_stroke_context(canvas_bounds(bounds), modifiers) else {
            stroke.cancel();
            return true;
        };
        let Some(point) = stroke_point(context, point[0], point[1], false) else {
            stroke.cancel();
            return true;
        };
        let pending = self.edit_pending();
        if let Err(error) = stroke.update(context, pending, point) {
            if error != midi_stroke::StrokeError::InvalidContext {
                self.error = Some(stroke_error(error).into());
            }
            stroke.cancel();
            return true;
        }
        // finish retires the working stroke and clears its own preview. Keep
        // the final shared geometry before that, then transfer only on admission.
        let preview = stroke.shared_preview();
        match stroke.finish(context, pending) {
            Ok(Some(command)) => {
                self.submit_midi_drag_preview(
                    context.clip,
                    super::midi_pending_drag::Frozen::Stroke(preview),
                    command,
                );
            }
            Ok(None) => {}
            Err(error) => self.error = Some(stroke_error(error).into()),
        }
        true
    }

    pub(super) fn cancel_midi_stroke(&mut self) -> bool {
        if let Some(mut stroke) = self.midi_stroke.take() {
            stroke.cancel();
            true
        } else {
            false
        }
    }

    pub(super) fn current_stroke_context(
        &self,
        bounds: midi_stroke::CanvasBounds,
        modifiers: StrokeModifiers,
    ) -> Option<StrokeContext> {
        let state = self.state.as_ref()?;
        let clip_id = self.detail_clip_id()?;
        let clip = state.project.midi_source(clip_id)?;
        Some(StrokeContext {
            generation: self.project_generation,
            revision: state.revision,
            clip: clip_id,
            clip_length: clip.source_length().raw(),
            minimum_gate: self.midi_minimum_creation_gate()?,
            grid: self.midi_snap_grid,
            effective_grid: midi_drag::effective_grid(self.midi_snap_grid, modifiers.snap),
            snap_modifier: modifiers.snap,
            pitch_modifier: modifiers.pitch,
            view_start: self.piano_beat_start,
            view_span: self.piano_beat_span.raw(),
            pitch_low: self.piano_pitch_low,
            pitch_rows: self.piano_pitch_rows,
            bounds,
            pitch_locked: self.midi_draw_pitch_locked ^ modifiers.pitch,
            draw_mode: self.midi_draw_mode,
        })
    }
}

pub(super) fn same_rendered_frame(current: StrokeContext, rendered: StrokeContext) -> bool {
    current.generation == rendered.generation
        && current.revision == rendered.revision
        && current.clip == rendered.clip
        && current.clip_length == rendered.clip_length
        && current.grid == rendered.grid
        && current.view_start == rendered.view_start
        && current.view_span == rendered.view_span
        && current.pitch_low == rendered.pitch_low
        && current.pitch_rows == rendered.pitch_rows
        && current.bounds == rendered.bounds
        && current.pitch_locked == (rendered.pitch_locked ^ current.pitch_modifier)
        && current.draw_mode == rendered.draw_mode
}

pub(super) fn canvas_bounds(bounds: [f32; 4]) -> midi_stroke::CanvasBounds {
    midi_stroke::CanvasBounds {
        x: bounds[0],
        y: bounds[1],
        width: bounds[2],
        height: bounds[3],
    }
}

fn stroke_point(
    context: StrokeContext,
    x: f32,
    y: f32,
    clamp_to_bounds: bool,
) -> Option<midi_stroke::StrokePoint> {
    let bounds = context.bounds;
    if !bounds.is_valid() || !x.is_finite() || !y.is_finite() {
        return None;
    }
    let mut local_x = x - bounds.x;
    let mut local_y = y - bounds.y;
    if clamp_to_bounds {
        let right = f32::from_bits((bounds.width).to_bits().saturating_sub(1));
        let bottom = f32::from_bits((bounds.height).to_bits().saturating_sub(1));
        local_x = local_x.clamp(0.0, right);
        local_y = local_y.clamp(0.0, bottom);
    } else if local_x < 0.0 || local_x >= bounds.width || local_y < 0.0 || local_y >= bounds.height
    {
        return None;
    }
    let fraction = f64::from(local_x / bounds.width);
    let offset = (fraction * context.view_span as f64) as u64;
    let beat_raw = u64::try_from(context.view_start.checked_add(i128::from(offset))?).ok()?;
    let length = context.authoring_length()?;
    let beat_raw = if clamp_to_bounds {
        beat_raw.min(length.checked_sub(1)?)
    } else {
        if beat_raw >= length {
            return None;
        }
        beat_raw
    };
    let row = ((local_y / bounds.height * f32::from(context.pitch_rows)).floor() as u8)
        .min(context.pitch_rows.checked_sub(1)?);
    let pitch = context
        .pitch_low
        .checked_add(context.pitch_rows.checked_sub(1)?)?
        .checked_sub(row)?;
    Some(midi_stroke::StrokePoint { beat_raw, pitch })
}

fn stroke_error(error: midi_stroke::StrokeError) -> &'static str {
    match error {
        midi_stroke::StrokeError::InvalidContext => "midi draw canceled · editor context changed",
        midi_stroke::StrokeError::InvalidPoint => "midi draw canceled · pointer left the roll",
        midi_stroke::StrokeError::InvalidBaseline => "midi draw canceled · note data changed",
        midi_stroke::StrokeError::TooManyNotes => "midi draw canceled · note limit reached",
        midi_stroke::StrokeError::TooManyCells => "midi draw canceled · stroke limit reached",
        midi_stroke::StrokeError::TooMuchBaselineWork => {
            "midi draw canceled · overlap work limit reached"
        }
        midi_stroke::StrokeError::Allocation => "midi draw canceled · memory is unavailable",
    }
}

pub(super) fn created_range_ids(
    selected_clip: Option<ClipId>,
    created_clip: ClipId,
    notes: &[MidiNote],
    first: NoteId,
    last: NoteId,
) -> Result<Option<Vec<NoteId>>, &'static str> {
    if selected_clip != Some(created_clip) {
        return Ok(None);
    }
    let range_len = last
        .value()
        .checked_sub(first.value())
        .and_then(|count| count.checked_add(1))
        .and_then(|count| usize::try_from(count).ok())
        .filter(|count| *count <= hex_project::MAX_NOTES)
        .ok_or("created midi note range is invalid")?;
    let mut created = Vec::new();
    created
        .try_reserve_exact(range_len)
        .map_err(|_| "could not reserve created midi selection")?;
    created.extend(notes.iter().filter_map(|note| {
        let id = note.id();
        (id >= first && id <= last).then_some(id)
    }));
    if created.len() != range_len {
        return Err("created midi note range is incomplete");
    }
    Ok(Some(created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn draw_coordinate_mapping_clamps_during_motion_and_rejects_offroll_release() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "draw".into(),
            })
            .unwrap()
        else {
            panic!("track expected")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        let context = StrokeContext {
            generation: 1,
            revision: 2,
            clip,
            clip_length: BeatTime::ONE.raw() * 4,
            minimum_gate: 1,
            grid: midi_drag::MidiGrid::Sixteenth,
            effective_grid: midi_drag::MidiGrid::Sixteenth,
            snap_modifier: false,
            pitch_modifier: false,
            view_start: 0,
            view_span: BeatTime::ONE.raw() * 4,
            pitch_low: 48,
            pitch_rows: 24,
            bounds: midi_stroke::CanvasBounds {
                x: 10.0,
                y: 20.0,
                width: 400.0,
                height: 240.0,
            },
            pitch_locked: true,
            draw_mode: true,
        };
        let clamped = stroke_point(context, 410.0, 260.0, true).unwrap();
        assert!(clamped.beat_raw < context.clip_length);
        assert!(clamped.beat_raw > context.clip_length - BeatTime::ONE.raw() / 1000);
        assert_eq!(stroke_point(context, 410.0, 260.0, true).unwrap().pitch, 48);
        assert!(stroke_point(context, 410.0, 260.0, false).is_none());
        assert!(stroke_point(context, f32::NAN, 50.0, true).is_none());
    }

    #[test]
    fn created_range_selects_only_ids_in_the_current_clip() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "draw".into(),
            })
            .unwrap()
        else {
            panic!("track expected")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        let mut insert = Vec::new();
        for pitch in [60, 61, 62] {
            insert.push(hex_project::MidiNoteInsert {
                active: true,
                start: BeatTime::from_quarters(pitch as u64 - 60).unwrap(),
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 100,
            });
        }
        let Created::Notes { first, last, .. } = editor
            .execute(Command::AddMidiNotes {
                clip,
                notes: insert,
            })
            .unwrap()
        else {
            panic!("notes range expected")
        };
        let notes = editor.project().midi_clip(clip).unwrap().notes();
        let selected = created_range_ids(Some(clip), clip, notes, first, last)
            .unwrap()
            .unwrap();
        assert_eq!(selected.len(), 3);
        assert_eq!(selected.first(), Some(&first));
        assert_eq!(selected.last(), Some(&last));
        assert_eq!(
            created_range_ids(None, clip, notes, first, last).unwrap(),
            None
        );
    }
}
