//! Source-local time selection shares ruler geometry and the existing edit-reply
//! selection owner. No Arrangement selection reflection or playback seek occurs.
use super::*;
use hex_project::BeatTime;

#[derive(Clone, Copy)]
struct Selection {
    generation: u64,
    clip: ClipId,
    start: u64,
    end: u64,
}
#[derive(Clone, Copy)]
pub(super) struct Capture {
    context: Context,
    note_epoch: u64,
    down_x: f32,
    last_x: f32,
    invert: bool,
}
#[derive(Default)]
pub(in crate::editor) struct State {
    selection: Option<Selection>,
    pub(super) gesture: Option<Capture>,
    epoch: u64,
}
impl State {
    pub(in crate::editor) fn cancel(&mut self) -> bool {
        self.gesture.take().is_some()
    }
    fn set(&mut self, selection: Selection) -> Result<(), &'static str> {
        let next = self
            .epoch
            .checked_add(1)
            .ok_or("time selection identity exhausted")?;
        self.selection = Some(selection);
        self.epoch = next;
        Ok(())
    }
    fn clear(&mut self) -> bool {
        let changed = self.selection.take().is_some();
        if changed {
            self.epoch = self.epoch.saturating_add(1);
        }
        changed
    }
}
fn beat_at(context: Context, x: f32, invert: bool) -> Option<u64> {
    let raw = midi_drag::beat_at_point(
        x,
        [
            context.bounds.x,
            context.bounds.y,
            context.bounds.width,
            context.bounds.height,
        ],
        context.start,
        context.span,
    )?;
    let grid = midi_drag::effective_grid(context.grid, invert);
    Some(clamp_loop_grid(
        midi_drag::snap_raw(raw, grid),
        0,
        super::super::midi_scene::zoom_limit(context.values.source_length),
        grid,
    ))
}
impl Capture {
    fn interval(self) -> Option<(u64, u64)> {
        if (self.last_x - self.down_x).abs() < 3.0 {
            return None;
        }
        let first = beat_at(self.context, self.down_x, self.invert)?;
        let last = beat_at(self.context, self.last_x, self.invert)?;
        (first != last).then_some((first.min(last), first.max(last)))
    }
}
impl EditorView {
    pub(in crate::editor) fn clear_midi_time_selection(&mut self) -> bool {
        self.midi_time_selection.clear()
    }
    pub(in crate::editor) fn midi_time_selection_epoch_matches(&self, epoch: u64) -> bool {
        self.midi_time_selection.epoch == epoch
            && self.midi_time_selection.selection.is_some_and(|s| {
                s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip)
            })
    }
    pub(in crate::editor) fn midi_time_copy_epoch(
        &self,
        clip: ClipId,
        start: u64,
        end: u64,
    ) -> Option<u64> {
        let s = self.midi_time_selection.selection?;
        (s.generation == self.project_generation
            && s.clip == clip
            && s.start == start
            && s.end == end)
            .then_some(self.midi_time_selection.epoch)
    }
    pub(in crate::editor) fn apply_midi_time_copy(&mut self, clip: ClipId, start: u64, end: u64) {
        if self.detail_clip_id() != Some(clip) {
            return;
        }
        if let Err(error) = self.midi_time_selection.set(Selection {
            generation: self.project_generation,
            clip,
            start,
            end,
        }) {
            self.error = Some(error.into());
            return;
        }
        self.midi_selection.clear();
        self.midi_marker = Some(start);
        // A successful duplicate must remain visible even after it grows beyond
        // the old viewport. Move only as far as needed; this is view state.
        let span = self.piano_beat_span.raw().max(end - start);
        let old = self.piano_beat_start.raw();
        let reveal = if start < old {
            start
        } else if end > old.saturating_add(span) {
            end.saturating_sub(span)
        } else {
            old
        };
        if let Some(source) = self.midi_clip() {
            let length = source.source_length().raw();
            self.piano_beat_span = BeatTime::from_raw(span);
            self.piano_beat_start =
                BeatTime::from_raw(super::super::midi_scene::clamp_start(reveal, length, span));
        }
    }
    pub(in crate::editor) fn midi_time_selection_bounds(&self) -> Option<(u64, u64)> {
        if let Some(capture) = self.midi_time_selection.gesture {
            return capture.interval();
        }
        let s = self.midi_time_selection.selection?;
        (s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip))
            .then_some((s.start, s.end))
    }
    pub(in crate::editor) fn duplicate_midi_time_selection(&mut self) -> bool {
        let Some(s) = self.midi_time_selection.selection.filter(|s| {
            s.generation == self.project_generation && self.detail_clip_id() == Some(s.clip)
        }) else {
            return false;
        };
        if self.edit_pending() {
            return true;
        }
        self.cancel_midi_note_gesture();
        self.enqueue(Request::Edit(
            hex_project::Command::DuplicateMidiSourceRange {
                clip: s.clip,
                start: BeatTime::from_raw(s.start),
                end: BeatTime::from_raw(s.end),
            },
        ));
        true
    }
    pub(super) fn begin_midi_time_selection(
        &mut self,
        context: Context,
        x: f32,
        invert: bool,
    ) -> bool {
        if !x.is_finite()
            || self.midi_ruler_context(context.bounds) != Some(context)
            || beat_at(context, x, invert).is_none()
        {
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_time_selection.gesture = Some(Capture {
            context,
            note_epoch: self.midi_selection.epoch(),
            down_x: x,
            last_x: x,
            invert,
        });
        true
    }
    pub(super) fn update_midi_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
    ) -> bool {
        let Some(capture) = self.midi_time_selection.gesture else {
            return false;
        };
        if self.midi_ruler_context(bounds) != Some(capture.context)
            || self.midi_selection.epoch() != capture.note_epoch
            || !x.is_finite()
        {
            return self.midi_time_selection.cancel();
        }
        let mut next = capture;
        next.last_x = x;
        next.invert = invert;
        self.midi_time_selection.gesture = Some(next);
        true
    }
    pub(super) fn finish_midi_time_selection(
        &mut self,
        bounds: CanvasBounds,
        x: f32,
        invert: bool,
    ) {
        self.update_midi_time_selection(bounds, x, invert);
        let Some(capture) = self.midi_time_selection.gesture.take() else {
            return;
        };
        let Some((start, end)) = capture.interval() else {
            return;
        };
        if let Err(error) = self.midi_time_selection.set(Selection {
            generation: self.project_generation,
            clip: capture.context.clip,
            start,
            end,
        }) {
            self.error = Some(error.into());
            return;
        }
        self.midi_selection.clear();
        self.midi_marker = Some(start);
    }
}
