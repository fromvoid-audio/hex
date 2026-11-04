//! Pure, bounded continuous MIDI drawing resolution.
//!
//! Hex's off-grid quarter-beat lattice and lowest-ID baseline overlap choice
//! are explicit stroke policies; neither is claimed as measured Live behavior.
use super::midi_drag::{MidiGrid, creation_gate, creation_start};
use hex_project::{ClipId, Command, MAX_NOTES, MidiNoteInsert, NoteId};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const MAX_STROKE_CELLS: usize = MAX_NOTES;
const MAX_BASELINE_PROBES: usize = MAX_NOTES * 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CanvasBounds {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl CanvasBounds {
    pub(super) fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StrokeContext {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) clip_length: u64,
    pub(super) minimum_gate: u64,
    pub(super) grid: MidiGrid,
    pub(super) effective_grid: MidiGrid,
    pub(super) snap_modifier: bool,
    pub(super) pitch_modifier: bool,
    pub(super) view_start: u64,
    pub(super) view_span: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
    pub(super) bounds: CanvasBounds,
    pub(super) pitch_locked: bool,
    pub(super) draw_mode: bool,
}

impl StrokeContext {
    /// Authoring may reveal time beyond the source. This bounds pointer-derived
    /// previews only; canonical growth occurs once, through the typed edit.
    pub(super) fn authoring_length(self) -> Option<u64> {
        self.view_start
            .checked_add(self.view_span)
            .map(|end| end.max(self.clip_length))
    }

    fn is_valid(self) -> bool {
        self.minimum_gate > 0
            && self.view_span > 0
            && self.clip_length > 0
            && self.view_span <= super::midi_scene::zoom_limit(self.clip_length)
            && self.view_start == super::midi_scene::clamp_start(self.view_start, self.view_span)
            && self.view_start.checked_add(self.view_span).is_some()
            && super::midi_scene::valid_pitch_range(self.pitch_low, self.pitch_rows)
            && self.bounds.is_valid()
            && self.draw_mode
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StrokePoint {
    pub(super) beat_raw: u64,
    pub(super) pitch: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BaselineNote {
    pub(super) id: NoteId,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) pitch: u8,
    pub(super) channel: u8,
    pub(super) velocity: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StrokeError {
    InvalidContext,
    InvalidPoint,
    InvalidBaseline,
    TooManyNotes,
    TooManyCells,
    TooMuchBaselineWork,
    Allocation,
}

#[derive(Default)]
pub(super) struct StrokePreview {
    pub(super) removed: Vec<NoteId>,
    pub(super) inserted: Vec<MidiNoteInsert>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Cell {
    time: i64,
    pitch: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interval {
    id: NoteId,
    start: u64,
    end: u64,
    pitch: u8,
}

struct BaselineIndex {
    intervals: Vec<Interval>,
    prefix_max_end: Vec<u64>,
    pitch_ranges: [(usize, usize); 128],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Remove(NoteId),
    Insert(MidiNoteInsert),
}

pub(super) struct MidiStroke {
    context: StrokeContext,
    authoring_length: u64,
    intervals: Vec<Interval>,
    prefix_max_end: Vec<u64>,
    pitch_ranges: [(usize, usize); 128],
    anchor: u64,
    step: u64,
    down_pitch: u8,
    last_cell: Cell,
    active: HashMap<Cell, Action>,
    baseline_cache: HashMap<Cell, Option<NoteId>>,
    down_hit: Option<NoteId>,
    initial_cell: Cell,
    preview: Arc<StrokePreview>,
    preview_version: u64,
    traversed: usize,
    baseline_probes: usize,
    cancelled: bool,
}

impl MidiStroke {
    #[cfg(test)]
    pub(super) fn begin(
        context: StrokeContext,
        clip_length: u64,
        baseline: Arc<[BaselineNote]>,
        down: StrokePoint,
    ) -> Result<Self, StrokeError> {
        Self::begin_with_hit(context, clip_length, baseline, down, None)
    }

    pub(super) fn begin_with_hit(
        context: StrokeContext,
        clip_length: u64,
        baseline: Arc<[BaselineNote]>,
        down: StrokePoint,
        down_hit: Option<NoteId>,
    ) -> Result<Self, StrokeError> {
        if !context.is_valid() || clip_length == 0 || context.clip_length != clip_length {
            return Err(StrokeError::InvalidContext);
        }
        let clip_length = context
            .authoring_length()
            .ok_or(StrokeError::InvalidContext)?;
        validate_point(down, clip_length)?;
        if baseline.len() > MAX_NOTES {
            return Err(StrokeError::TooManyNotes);
        }

        let step = context
            .effective_grid
            .step_raw()
            .unwrap_or_else(|| context.effective_grid.movement_step());
        if step == 0 {
            return Err(StrokeError::InvalidContext);
        }
        let anchor = match context.effective_grid.step_raw() {
            Some(step) => {
                let last_grid = (clip_length - 1) / step * step;
                creation_start(down.beat_raw, context.effective_grid).min(last_grid)
            }
            None => down.beat_raw,
        };
        let initial_cell = Cell {
            time: 0,
            pitch: down.pitch,
        };
        let index = build_interval_index(&baseline, context.clip_length)?;
        let mut stroke = Self {
            context,
            authoring_length: clip_length,
            intervals: index.intervals,
            prefix_max_end: index.prefix_max_end,
            pitch_ranges: index.pitch_ranges,
            anchor,
            step,
            down_pitch: down.pitch,
            last_cell: initial_cell,
            active: HashMap::new(),
            baseline_cache: HashMap::new(),
            down_hit,
            initial_cell,
            preview: Arc::new(StrokePreview::default()),
            preview_version: 0,
            traversed: 1,
            baseline_probes: 0,
            cancelled: false,
        };
        if let Some(id) = down_hit {
            if !stroke.baseline_note_has_pitch(id, initial_cell.pitch) {
                return Err(StrokeError::InvalidBaseline);
            }
        }
        stroke
            .baseline_cache
            .try_reserve(1)
            .map_err(|_| StrokeError::Allocation)?;
        // The original down hit is authoritative. An empty grid down inserts
        // even if its snapped cell overlaps an authored short note.
        stroke.baseline_cache.insert(initial_cell, down_hit);
        stroke.add_cell(initial_cell)?;
        stroke.refresh_preview()?;
        Ok(stroke)
    }

    pub(super) fn update(
        &mut self,
        context: StrokeContext,
        pending: bool,
        point: StrokePoint,
    ) -> Result<bool, StrokeError> {
        if self.cancelled {
            return Ok(false);
        }
        if !self.context_matches(context, pending) {
            self.cancelled = true;
            return Err(StrokeError::InvalidContext);
        }
        if let Err(error) = validate_point(point, self.authoring_length) {
            self.cancelled = true;
            return Err(error);
        }

        let previous_version = self.preview_version;
        let result = self.point_cell(point).and_then(|next| {
            if self.context.pitch_locked {
                self.update_locked(next.time)?;
            } else {
                self.update_melodic(next)?;
            }
            self.last_cell = next;
            Ok(())
        });
        if let Err(error) = result {
            self.cancel();
            return Err(error);
        }
        if self.preview_version != previous_version {
            if let Err(error) = self.refresh_preview() {
                self.cancel();
                return Err(error);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Immutable frame and down anchor retained across preview/traversal updates.
    pub(super) fn admission(&self) -> (StrokeContext, u64, u8) {
        (self.context, self.anchor, self.down_pitch)
    }

    pub(super) fn shared_preview(&self) -> Arc<StrokePreview> {
        Arc::clone(&self.preview)
    }

    pub(super) fn minimum_gate_for(&self, generation: u64, revision: u64) -> Option<u64> {
        (self.context.generation == generation && self.context.revision == revision)
            .then_some(self.context.minimum_gate)
    }

    pub(super) fn modifiers_match(&self, snap: bool, pitch: bool) -> bool {
        self.context.snap_modifier == snap && self.context.pitch_modifier == pitch
    }

    pub(super) fn cancel(&mut self) {
        self.cancelled = true;
        self.active.clear();
        self.preview = Arc::new(StrokePreview::default());
    }

    pub(super) fn finish(
        &mut self,
        context: StrokeContext,
        pending: bool,
    ) -> Result<Option<Command>, StrokeError> {
        if self.cancelled || !self.context_matches(context, pending) {
            self.cancelled = true;
            self.active.clear();
            return Ok(None);
        }
        let output = self.build_command();
        self.cancelled = true;
        self.active.clear();
        self.preview = Arc::new(StrokePreview::default());
        output
    }

    fn build_command(&mut self) -> Result<Option<Command>, StrokeError> {
        let mut ordered = Vec::new();
        if ordered.try_reserve_exact(self.active.len()).is_err() {
            return Err(StrokeError::Allocation);
        }
        ordered.extend(self.active.iter().map(|(cell, action)| (*cell, *action)));
        ordered.sort_unstable_by_key(|(cell, _)| (cell.time, cell.pitch));

        let mut remove = Vec::new();
        let mut insert = Vec::new();
        if remove.try_reserve_exact(ordered.len()).is_err()
            || insert.try_reserve_exact(ordered.len()).is_err()
        {
            return Err(StrokeError::Allocation);
        }
        for (_, action) in ordered {
            match action {
                Action::Remove(id) => remove.push(id),
                Action::Insert(note) => insert.push(note),
            }
        }
        remove.sort_unstable();
        remove.dedup();
        if remove.len() > MAX_NOTES || insert.len() > MAX_NOTES {
            return Err(StrokeError::TooManyNotes);
        }
        if remove.is_empty() && insert.is_empty() {
            return Ok(None);
        }
        let grows = insert.iter().any(|note| {
            note.start.raw().saturating_add(note.duration.raw()) > self.context.clip_length
        });
        Ok(Some(if grows {
            Command::EditMidiNotesWithSourceExtent {
                clip: self.context.clip,
                remove,
                insert,
            }
        } else {
            Command::EditMidiNotes {
                clip: self.context.clip,
                remove,
                insert,
            }
        }))
    }

    fn context_matches(&self, context: StrokeContext, pending: bool) -> bool {
        !pending && context.is_valid() && context == self.context
    }

    fn point_cell(&self, point: StrokePoint) -> Result<Cell, StrokeError> {
        let delta = i128::from(point.beat_raw) - i128::from(self.anchor);
        let step = i128::from(self.step);
        let index = if self.context.effective_grid.step_raw().is_some() {
            // Every snapped painted onset belongs to the cell at the pointer's
            // left. In particular releasing inside the initial cell adds no
            // second note. The explicit off-grid stroke lattice stays nearest.
            delta.div_euclid(step)
        } else {
            let magnitude = delta.unsigned_abs();
            let divisor = u128::from(self.step);
            let quotient = magnitude / divisor;
            let remainder = magnitude % divisor;
            let rounded = quotient + u128::from(remainder.saturating_mul(2) >= divisor);
            let signed = i128::try_from(rounded).map_err(|_| StrokeError::TooManyCells)?;
            if delta < 0 { -signed } else { signed }
        };
        let min_index = ceil_div(-i128::from(self.anchor), step);
        let max_index =
            (i128::from(self.authoring_length - 1) - i128::from(self.anchor)).div_euclid(step);
        let clamped = index.clamp(min_index, max_index);
        let time = i64::try_from(clamped).map_err(|_| StrokeError::TooManyCells)?;
        Ok(Cell {
            time,
            pitch: if self.context.pitch_locked {
                self.down_pitch
            } else {
                point.pitch
            },
        })
    }

    fn update_locked(&mut self, next_time: i64) -> Result<(), StrokeError> {
        let previous = self.last_cell.time;
        if previous == next_time {
            return Ok(());
        }
        let distance = previous.abs_diff(next_time);
        self.add_traversal(usize::try_from(distance).map_err(|_| StrokeError::TooManyCells)?)?;
        let pitch = self.down_pitch;
        let low = next_time.min(0);
        let high = next_time.max(0);
        let previous_len = self.active.len();
        self.active
            .retain(|cell, _| cell.pitch == pitch && cell.time >= low && cell.time <= high);
        if self.active.len() != previous_len {
            self.mark_preview_changed()?;
        }

        let previous_abs = previous.unsigned_abs();
        let next_abs = next_time.unsigned_abs();
        let should_add = next_time != 0
            && (previous == 0
                || previous.signum() != next_time.signum()
                || next_abs > previous_abs);
        if should_add {
            let first = if previous != 0 && previous.signum() == next_time.signum() {
                i64::try_from(
                    previous_abs
                        .checked_add(1)
                        .ok_or(StrokeError::TooManyCells)?,
                )
                .map_err(|_| StrokeError::TooManyCells)?
            } else {
                1
            };
            let last = i64::try_from(next_abs).map_err(|_| StrokeError::TooManyCells)?;
            for magnitude in first..=last {
                let time = if next_time < 0 { -magnitude } else { magnitude };
                self.add_cell(Cell { time, pitch })?;
            }
        }
        Ok(())
    }

    fn update_melodic(&mut self, next: Cell) -> Result<(), StrokeError> {
        if self.last_cell == next {
            return Ok(());
        }
        let dx = (i128::from(next.time) - i128::from(self.last_cell.time)).unsigned_abs();
        let dy = u128::from(next.pitch.abs_diff(self.last_cell.pitch));
        let major = dx.max(dy);
        self.add_traversal(usize::try_from(major).map_err(|_| StrokeError::TooManyCells)?)?;
        if dx >= dy {
            for step in 0..=dx {
                let time = interpolate_i64(self.last_cell.time, next.time, step, dx)?;
                let pitch = interpolate_u8(self.last_cell.pitch, next.pitch, step, dx)?;
                self.add_cell(Cell { time, pitch })?;
            }
        } else {
            for step in 0..=dy {
                let time = interpolate_i64(self.last_cell.time, next.time, step, dy)?;
                let pitch = interpolate_u8(self.last_cell.pitch, next.pitch, step, dy)?;
                self.add_cell(Cell { time, pitch })?;
            }
        }
        Ok(())
    }

    fn add_cell(&mut self, cell: Cell) -> Result<(), StrokeError> {
        if self.active.contains_key(&cell) {
            return Ok(());
        }
        if self.active.len() >= MAX_STROKE_CELLS {
            return self.cancel_with(StrokeError::TooManyCells);
        }
        let Some(onset) = self.cell_onset(cell.time) else {
            return Ok(());
        };
        let Some(existing) = self.baseline_hit(cell, onset)? else {
            let remaining = self.authoring_length - onset;
            let Some(duration) = creation_gate(
                remaining,
                self.context.effective_grid,
                self.context.view_span,
                self.context.bounds.width,
                self.context.minimum_gate,
            ) else {
                return self.cancel_with(StrokeError::InvalidPoint);
            };
            let note = MidiNoteInsert {
                active: true,
                start: hex_project::BeatTime::from_raw(onset),
                duration: hex_project::BeatTime::from_raw(duration),
                pitch: cell.pitch,
                channel: 0,
                velocity: 100,
            };
            self.active
                .try_reserve(1)
                .map_err(|_| StrokeError::Allocation)?;
            self.active.insert(cell, Action::Insert(note));
            self.mark_preview_changed()?;
            return Ok(());
        };
        self.active
            .try_reserve(1)
            .map_err(|_| StrokeError::Allocation)?;
        self.active.insert(cell, Action::Remove(existing));
        self.mark_preview_changed()?;
        Ok(())
    }

    fn mark_preview_changed(&mut self) -> Result<(), StrokeError> {
        self.preview_version = self
            .preview_version
            .checked_add(1)
            .ok_or(StrokeError::TooManyCells)?;
        Ok(())
    }

    fn refresh_preview(&mut self) -> Result<(), StrokeError> {
        let mut preview = StrokePreview::default();
        preview
            .removed
            .try_reserve_exact(self.active.len())
            .map_err(|_| StrokeError::Allocation)?;
        preview
            .inserted
            .try_reserve_exact(self.active.len())
            .map_err(|_| StrokeError::Allocation)?;
        for action in self.active.values() {
            match action {
                Action::Remove(id) => preview.removed.push(*id),
                Action::Insert(note) => preview.inserted.push(*note),
            }
        }
        preview.removed.sort_unstable();
        preview.removed.dedup();
        preview
            .inserted
            .sort_unstable_by_key(|note| (note.start.raw(), note.pitch));
        self.preview = Arc::new(preview);
        Ok(())
    }

    fn baseline_note_has_pitch(&self, id: NoteId, pitch: u8) -> bool {
        let (start, stop) = self.pitch_ranges[usize::from(pitch)];
        self.intervals[start..stop].iter().any(|note| note.id == id)
    }

    fn baseline_hit(&mut self, cell: Cell, onset: u64) -> Result<Option<NoteId>, StrokeError> {
        if let Some(cached) = self.baseline_cache.get(&cell) {
            return Ok(*cached);
        }
        if cell == self.initial_cell
            && let Some(id) = self.down_hit
        {
            return Ok(Some(id));
        }
        let end = onset.saturating_add(self.step).min(self.authoring_length);
        let (start, stop) = self.pitch_ranges[usize::from(cell.pitch)];
        let range = &self.intervals[start..stop];
        let before_end = range.partition_point(|item| item.start < end) + start;
        let mut best = None;
        let mut index = before_end;
        while index > start {
            index -= 1;
            if self.prefix_max_end[index] <= onset {
                break;
            }
            self.baseline_probes = self
                .baseline_probes
                .checked_add(1)
                .ok_or(StrokeError::TooMuchBaselineWork)?;
            if self.baseline_probes > MAX_BASELINE_PROBES {
                return self.cancel_with(StrokeError::TooMuchBaselineWork);
            }
            let item = self.intervals[index];
            if item.end > onset && item.start < end && best.is_none_or(|id| item.id < id) {
                best = Some(item.id);
            }
        }
        self.baseline_cache
            .try_reserve(1)
            .map_err(|_| StrokeError::Allocation)?;
        self.baseline_cache.insert(cell, best);
        Ok(best)
    }

    fn cell_onset(&self, time: i64) -> Option<u64> {
        let raw = i128::from(self.anchor) + i128::from(time) * i128::from(self.step);
        let onset = u64::try_from(raw).ok()?;
        (onset < self.authoring_length).then_some(onset)
    }

    fn add_traversal(&mut self, count: usize) -> Result<(), StrokeError> {
        self.traversed = self
            .traversed
            .checked_add(count)
            .ok_or(StrokeError::TooManyCells)?;
        if self.traversed > MAX_STROKE_CELLS {
            return self.cancel_with(StrokeError::TooManyCells);
        }
        Ok(())
    }

    fn cancel_with<T>(&mut self, error: StrokeError) -> Result<T, StrokeError> {
        self.cancel();
        Err(error)
    }
}

fn validate_point(point: StrokePoint, clip_length: u64) -> Result<(), StrokeError> {
    if point.beat_raw >= clip_length || point.pitch > 127 {
        Err(StrokeError::InvalidPoint)
    } else {
        Ok(())
    }
}

fn build_interval_index(
    baseline: &[BaselineNote],
    clip_length: u64,
) -> Result<BaselineIndex, StrokeError> {
    let mut intervals = Vec::new();
    intervals
        .try_reserve_exact(baseline.len())
        .map_err(|_| StrokeError::Allocation)?;
    let mut ids = HashSet::new();
    ids.try_reserve(baseline.len())
        .map_err(|_| StrokeError::Allocation)?;
    for note in baseline {
        if !ids.insert(note.id) {
            return Err(StrokeError::InvalidBaseline);
        }
        if note.pitch > 127
            || note.channel > 15
            || !(1..=127).contains(&note.velocity)
            || note.duration == 0
        {
            return Err(StrokeError::InvalidBaseline);
        }
        let end = note
            .start
            .checked_add(note.duration)
            .filter(|end| *end <= clip_length)
            .ok_or(StrokeError::InvalidBaseline)?;
        intervals.push(Interval {
            id: note.id,
            start: note.start,
            end,
            pitch: note.pitch,
        });
    }
    intervals.sort_unstable_by_key(|item| (item.pitch, item.start, item.id));
    let mut prefix_max_end = Vec::new();
    prefix_max_end
        .try_reserve_exact(intervals.len())
        .map_err(|_| StrokeError::Allocation)?;
    let mut pitch_ranges = [(0, 0); 128];
    let mut cursor = 0;
    while cursor < intervals.len() {
        let pitch = intervals[cursor].pitch;
        let start = cursor;
        let mut max_end = 0;
        while cursor < intervals.len() && intervals[cursor].pitch == pitch {
            max_end = max_end.max(intervals[cursor].end);
            prefix_max_end.push(max_end);
            cursor += 1;
        }
        pitch_ranges[usize::from(pitch)] = (start, cursor);
    }
    Ok(BaselineIndex {
        intervals,
        prefix_max_end,
        pitch_ranges,
    })
}

fn ceil_div(value: i128, divisor: i128) -> i128 {
    -(-value).div_euclid(divisor)
}

fn interpolate_i64(start: i64, end: i64, step: u128, total: u128) -> Result<i64, StrokeError> {
    if total == 0 {
        return Ok(end);
    }
    let delta = i128::from(end) - i128::from(start);
    let magnitude = delta.unsigned_abs();
    let numerator = magnitude
        .checked_mul(step)
        .ok_or(StrokeError::TooManyCells)?;
    let rounded = numerator
        .checked_add(total / 2)
        .ok_or(StrokeError::TooManyCells)?
        / total;
    let rounded = i128::try_from(rounded).map_err(|_| StrokeError::TooManyCells)?;
    i64::try_from(i128::from(start) + if delta < 0 { -rounded } else { rounded })
        .map_err(|_| StrokeError::TooManyCells)
}

fn interpolate_u8(start: u8, end: u8, step: u128, total: u128) -> Result<u8, StrokeError> {
    interpolate_i64(i64::from(start), i64::from(end), step, total)
        .and_then(|value| u8::try_from(value).map_err(|_| StrokeError::InvalidPoint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Created, Editor, Project};

    fn beat(value: u64) -> u64 {
        BeatTime::from_quarters(value).unwrap().raw()
    }

    fn context(clip: ClipId, length: u64, grid: MidiGrid, locked: bool) -> StrokeContext {
        StrokeContext {
            generation: 4,
            revision: 9,
            clip,
            clip_length: length,
            minimum_gate: 1,
            grid,
            effective_grid: grid,
            snap_modifier: false,
            pitch_modifier: false,
            view_start: 0,
            view_span: length,
            pitch_low: 48,
            pitch_rows: 24,
            bounds: CanvasBounds {
                x: 0.0,
                y: 0.0,
                width: 880.0,
                height: 320.0,
            },
            pitch_locked: locked,
            draw_mode: true,
        }
    }

    fn fixture(length: u64) -> (Editor, ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "stroke".into(),
            })
            .unwrap()
        else {
            panic!("track expected")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(length),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        (editor, clip)
    }

    fn baseline(editor: &Editor, clip: ClipId) -> Arc<[BaselineNote]> {
        editor
            .project()
            .midi_clip(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| BaselineNote {
                id: note.id(),
                start: note.start().raw(),
                duration: note.duration().raw(),
                pitch: note.pitch(),
                channel: note.channel(),
                velocity: note.velocity(),
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn stroke(
        editor: &Editor,
        clip: ClipId,
        length: u64,
        grid: MidiGrid,
        locked: bool,
        down: StrokePoint,
    ) -> MidiStroke {
        MidiStroke::begin(
            context(clip, length, grid, locked),
            length,
            baseline(editor, clip),
            down,
        )
        .unwrap()
    }

    fn command_parts(command: Command) -> (ClipId, Vec<NoteId>, Vec<MidiNoteInsert>) {
        let Command::EditMidiNotes {
            clip,
            remove,
            insert,
        } = command
        else {
            panic!("mixed edit expected")
        };
        (clip, remove, insert)
    }

    #[test]
    fn sparse_and_dense_melodic_straight_paths_resolve_identically() {
        let (editor, clip) = fixture(beat(8));
        let down = StrokePoint {
            beat_raw: 0,
            pitch: 60,
        };
        let mut sparse = stroke(&editor, clip, beat(8), MidiGrid::Sixteenth, false, down);
        sparse
            .update(
                context(clip, beat(8), MidiGrid::Sixteenth, false),
                false,
                StrokePoint {
                    beat_raw: beat(4),
                    pitch: 64,
                },
            )
            .unwrap();
        let sparse_command = sparse
            .finish(context(clip, beat(8), MidiGrid::Sixteenth, false), false)
            .unwrap()
            .unwrap();

        let mut dense = stroke(&editor, clip, beat(8), MidiGrid::Sixteenth, false, down);
        for step in 1..=16 {
            dense
                .update(
                    context(clip, beat(8), MidiGrid::Sixteenth, false),
                    false,
                    StrokePoint {
                        beat_raw: step * beat(1) / 4,
                        pitch: 60 + ((step + 2) / 4) as u8,
                    },
                )
                .unwrap();
        }
        let dense_command = dense
            .finish(context(clip, beat(8), MidiGrid::Sixteenth, false), false)
            .unwrap()
            .unwrap();
        let sparse_parts = command_parts(sparse_command);
        let dense_parts = command_parts(dense_command);
        assert_eq!(sparse_parts, dense_parts);
        let (_, remove, insert) = dense_parts;
        assert!(remove.is_empty());
        assert_eq!(insert.first().unwrap().start.raw(), 0);
        assert_eq!(insert.last().unwrap().start.raw(), beat(4));
    }

    #[test]
    fn lattice_aligned_negative_vertical_and_steep_paths_are_density_invariant() {
        let (editor, clip) = fixture(beat(8));
        let cases = [
            (
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 64,
                },
                StrokePoint {
                    beat_raw: 0,
                    pitch: 60,
                },
                vec![
                    StrokePoint {
                        beat_raw: beat(3) / 4,
                        pitch: 63,
                    },
                    StrokePoint {
                        beat_raw: beat(2) / 4,
                        pitch: 62,
                    },
                    StrokePoint {
                        beat_raw: beat(1) / 4,
                        pitch: 61,
                    },
                    StrokePoint {
                        beat_raw: 0,
                        pitch: 60,
                    },
                ],
            ),
            (
                StrokePoint {
                    beat_raw: beat(2),
                    pitch: 60,
                },
                StrokePoint {
                    beat_raw: beat(2),
                    pitch: 65,
                },
                (61..=65)
                    .map(|pitch| StrokePoint {
                        beat_raw: beat(2),
                        pitch,
                    })
                    .collect(),
            ),
            (
                StrokePoint {
                    beat_raw: beat(2),
                    pitch: 60,
                },
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 65,
                },
                vec![
                    StrokePoint {
                        beat_raw: beat(7) / 4,
                        pitch: 61,
                    },
                    StrokePoint {
                        beat_raw: beat(6) / 4,
                        pitch: 62,
                    },
                    StrokePoint {
                        beat_raw: beat(6) / 4,
                        pitch: 63,
                    },
                    StrokePoint {
                        beat_raw: beat(5) / 4,
                        pitch: 64,
                    },
                    StrokePoint {
                        beat_raw: beat(1),
                        pitch: 65,
                    },
                ],
            ),
        ];

        for (down, endpoint, dense_points) in cases {
            let grid = MidiGrid::Sixteenth;
            let mut sparse = stroke(&editor, clip, beat(8), grid, false, down);
            sparse
                .update(context(clip, beat(8), grid, false), false, endpoint)
                .unwrap();
            let sparse = command_parts(
                sparse
                    .finish(context(clip, beat(8), grid, false), false)
                    .unwrap()
                    .unwrap(),
            );

            let mut dense = stroke(&editor, clip, beat(8), grid, false, down);
            for point in dense_points {
                dense
                    .update(context(clip, beat(8), grid, false), false, point)
                    .unwrap();
            }
            let dense = command_parts(
                dense
                    .finish(context(clip, beat(8), grid, false), false)
                    .unwrap()
                    .unwrap(),
            );
            assert_eq!(dense, sparse);
        }
    }

    #[test]
    fn fixed_and_free_grid_keep_distinct_first_click_anchor_and_gate() {
        let (editor, clip) = fixture(beat(4));
        let down = StrokePoint {
            beat_raw: beat(1) / 8,
            pitch: 60,
        };
        let mut fixed = stroke(&editor, clip, beat(4), MidiGrid::Sixteenth, false, down);
        let fixed_command = fixed
            .finish(context(clip, beat(4), MidiGrid::Sixteenth, false), false)
            .unwrap()
            .unwrap();
        let (_, _, fixed_insert) = command_parts(fixed_command);
        assert_eq!(fixed_insert[0].start.raw(), 0);
        assert_eq!(fixed_insert[0].duration.raw(), beat(1) / 4);

        let mut free = stroke(&editor, clip, beat(4), MidiGrid::Off, false, down);
        let free_command = free
            .finish(context(clip, beat(4), MidiGrid::Off, false), false)
            .unwrap()
            .unwrap();
        let (_, _, free_insert) = command_parts(free_command);
        assert_eq!(free_insert[0].start.raw(), down.beat_raw);
        assert_eq!(
            free_insert[0].duration.raw(),
            (beat(4) as f64 / 880.0).ceil() as u64
        );

        let mut free_path_stroke = stroke(&editor, clip, beat(4), MidiGrid::Off, false, down);
        free_path_stroke
            .update(
                context(clip, beat(4), MidiGrid::Off, false),
                false,
                StrokePoint {
                    beat_raw: down.beat_raw + beat(1) / 4,
                    pitch: down.pitch,
                },
            )
            .unwrap();
        let free_path = free_path_stroke
            .finish(context(clip, beat(4), MidiGrid::Off, false), false)
            .unwrap()
            .unwrap();
        let (_, _, free_path_insert) = command_parts(free_path);
        assert_eq!(free_path_insert.len(), 2);
        assert_eq!(free_path_insert[0].start.raw(), down.beat_raw);
        assert_eq!(free_path_insert[1].start.raw(), down.beat_raw + beat(1) / 4);
    }

    #[test]
    fn clicked_short_off_grid_note_uses_scene_hit_but_empty_down_inserts() {
        let (mut editor, clip) = fixture(beat(4));
        let note_start = BeatTime::ONE.raw() / 5;
        let note_duration = BeatTime::ONE.raw() / 100;
        let Created::Note(note_id) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_raw(note_start),
                duration: BeatTime::from_raw(note_duration),
                pitch: 60,
                channel: 3,
                velocity: 77,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let down = StrokePoint {
            beat_raw: note_start + note_duration / 2,
            pitch: 60,
        };
        let context = context(clip, beat(4), MidiGrid::Sixteenth, false);
        let baseline = baseline(&editor, clip);
        let clicked = MidiStroke::begin_with_hit(
            context,
            beat(4),
            Arc::clone(&baseline),
            down,
            Some(note_id),
        )
        .unwrap();
        let clicked_preview = clicked.shared_preview();
        assert_eq!(clicked_preview.removed, [note_id]);
        assert!(clicked_preview.inserted.is_empty());

        let empty = MidiStroke::begin_with_hit(context, beat(4), baseline, down, None).unwrap();
        let empty_preview = empty.shared_preview();
        assert!(empty_preview.removed.is_empty());
        assert_eq!(empty_preview.inserted.len(), 1);
        assert_eq!(empty_preview.inserted[0].start.raw(), 0);
    }

    #[test]
    fn fixed_grid_anchor_clamps_to_last_grid_line_inside_clip() {
        let length = beat(1) + beat(1) * 3 / 5;
        let (editor, clip) = fixture(length);
        let stroke = stroke(
            &editor,
            clip,
            length,
            MidiGrid::Quarter,
            false,
            StrokePoint {
                beat_raw: length - 1,
                pitch: 60,
            },
        );
        let preview = stroke.shared_preview();
        assert_eq!(preview.inserted[0].start.raw(), beat(1));
    }

    #[test]
    fn erasing_multiple_cells_of_one_note_has_unique_valid_preview_and_command() {
        let (mut editor, clip) = fixture(beat(4));
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 2,
                velocity: 93,
            })
            .unwrap()
        else {
            panic!("note expected");
        };
        let c = context(clip, beat(4), MidiGrid::Sixteenth, false);
        let mut stroke = MidiStroke::begin_with_hit(
            c,
            beat(4),
            baseline(&editor, clip),
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
            Some(note),
        )
        .unwrap();
        stroke
            .update(
                c,
                false,
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 60,
                },
            )
            .unwrap();
        let preview = stroke.shared_preview();
        assert_eq!(preview.removed, [note]);
        assert!(preview.inserted.is_empty());
        assert!(
            editor
                .project()
                .midi_source(clip)
                .unwrap()
                .preview_note_overwrite(&preview.removed, &preview.inserted)
                .is_ok()
        );
        let command = stroke.finish(c, false).unwrap().unwrap();
        let (_, remove, insert) = command_parts(command.clone());
        assert_eq!(remove, preview.removed);
        assert!(insert.is_empty());
        editor.execute(command).unwrap();
        assert!(
            editor
                .project()
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        );
    }

    #[test]
    fn preview_snapshots_are_shared_until_a_cell_action_changes() {
        let (editor, clip) = fixture(beat(4));
        let grid = MidiGrid::Sixteenth;
        let mut stroke = stroke(
            &editor,
            clip,
            beat(4),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        let initial = stroke.shared_preview();
        assert_eq!(initial.inserted.len(), 1);
        assert!(
            !stroke
                .update(
                    context(clip, beat(4), grid, false),
                    false,
                    StrokePoint {
                        beat_raw: 1,
                        pitch: 60,
                    },
                )
                .unwrap()
        );
        assert!(Arc::ptr_eq(&initial, &stroke.shared_preview()));
        assert!(
            stroke
                .update(
                    context(clip, beat(4), grid, false),
                    false,
                    StrokePoint {
                        beat_raw: beat(1) / 4,
                        pitch: 60,
                    },
                )
                .unwrap()
        );
        let updated = stroke.shared_preview();
        assert!(!Arc::ptr_eq(&initial, &updated));
        assert_eq!(initial.inserted.len(), 1);
        assert_eq!(updated.inserted.len(), 2);
    }

    #[test]
    fn locked_backtracking_retracts_and_direction_reversal_rebuilds_path() {
        let (editor, clip) = fixture(beat(8));
        let grid = MidiGrid::Quarter;
        let mut stroke = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            true,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        for time in [beat(4), beat(2), beat(5), beat(1)] {
            stroke
                .update(
                    context(clip, beat(8), grid, true),
                    false,
                    StrokePoint {
                        beat_raw: time,
                        pitch: 100,
                    },
                )
                .unwrap();
        }
        let command = stroke
            .finish(context(clip, beat(8), grid, true), false)
            .unwrap()
            .unwrap();
        let (_, _, insert) = command_parts(command);
        assert_eq!(insert.len(), 2);
        assert!(insert.iter().all(|note| note.pitch == 60));
        assert_eq!(insert[0].start.raw(), 0);
        assert_eq!(insert[1].start.raw(), beat(1));
    }

    #[test]
    fn melodic_vertical_traversal_revisit_and_expression_preservation_are_deterministic() {
        let (mut editor, clip) = fixture(beat(4));
        let Created::Note(existing) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_ratio(1, 2).unwrap(),
                pitch: 60,
                channel: 9,
                velocity: 23,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let grid = MidiGrid::Off;
        let mut stroke = MidiStroke::begin_with_hit(
            context(clip, beat(4), grid, false),
            beat(4),
            baseline(&editor, clip),
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
            Some(existing),
        )
        .unwrap();
        for point in [
            StrokePoint {
                beat_raw: 0,
                pitch: 64,
            },
            StrokePoint {
                beat_raw: 0,
                pitch: 64,
            },
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        ] {
            stroke
                .update(context(clip, beat(4), grid, false), false, point)
                .unwrap();
        }
        let command = stroke
            .finish(context(clip, beat(4), grid, false), false)
            .unwrap()
            .unwrap();
        let (command_clip, remove, insert) = command_parts(command);
        assert_eq!(command_clip, clip);
        assert_eq!(remove, [existing]);
        assert_eq!(
            insert.iter().map(|note| note.pitch).collect::<Vec<_>>(),
            [61, 62, 63, 64]
        );
        assert!(insert.iter().all(|note| note.start.raw() == 0));

        let before = editor.project().clone();
        editor
            .execute(Command::EditMidiNotes {
                clip,
                remove,
                insert,
            })
            .unwrap();
        let after = editor.project().clone();
        assert!(editor.project().midi_note(existing).is_none());
        assert!(editor.undo());
        assert_eq!(editor.project().tracks(), before.tracks());
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }

    #[test]
    fn baseline_cell_overlap_chooses_lowest_id_and_free_gate_clamps_at_clip_end() {
        let (mut editor, clip) = fixture(beat(2));
        let Created::Note(first) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_ratio(3, 8).unwrap(),
                pitch: 60,
                channel: 1,
                velocity: 12,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let Created::Note(second) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_ratio(3, 8).unwrap(),
                duration: BeatTime::from_ratio(1, 8).unwrap(),
                pitch: 60,
                channel: 2,
                velocity: 99,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let grid = MidiGrid::Off;
        let mut erase = MidiStroke::begin_with_hit(
            context(clip, beat(2), grid, false),
            beat(2),
            baseline(&editor, clip),
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
            Some(first),
        )
        .unwrap();
        erase
            .update(
                context(clip, beat(2), grid, false),
                false,
                StrokePoint {
                    beat_raw: beat(1) / 4,
                    pitch: 60,
                },
            )
            .unwrap();
        let command = erase
            .finish(context(clip, beat(2), grid, false), false)
            .unwrap()
            .unwrap();
        let (_, remove, insert) = command_parts(command);
        assert_eq!(remove, [first]);
        assert!(insert.is_empty());
        editor
            .execute(Command::EditMidiNotes {
                clip,
                remove,
                insert,
            })
            .unwrap();
        assert!(editor.project().midi_note(first).is_none());
        let retained = editor.project().midi_note(second).unwrap();
        assert_eq!(retained.channel(), 2);
        assert_eq!(retained.velocity(), 99);

        let short_length = beat(1) / 8;
        let (editor, short_clip) = fixture(short_length);
        let down = StrokePoint {
            beat_raw: short_length - 1,
            pitch: 67,
        };
        let mut short = stroke(
            &editor,
            short_clip,
            short_length,
            MidiGrid::Off,
            false,
            down,
        );
        let command = short
            .finish(
                context(short_clip, short_length, MidiGrid::Off, false),
                false,
            )
            .unwrap()
            .unwrap();
        let (_, _, insert) = command_parts(command);
        assert_eq!(insert[0].start.raw(), short_length - 1);
        assert_eq!(insert[0].duration.raw(), 1);
    }

    #[test]
    fn stale_pending_invalid_and_over_cap_strokes_cannot_emit_partial_commands() {
        let (mut editor, clip) = fixture(beat(8));
        let Created::Note(baseline_id) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(1),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let grid = MidiGrid::Sixteenth;
        let mut stale = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        let mut changed = context(clip, beat(8), grid, false);
        changed.revision += 1;
        assert_eq!(
            stale.update(
                changed,
                false,
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 60
                }
            ),
            Err(StrokeError::InvalidContext)
        );
        assert!(
            stale
                .finish(context(clip, beat(8), grid, false), false)
                .unwrap()
                .is_none()
        );

        let mut pending = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        assert_eq!(
            pending.update(
                context(clip, beat(8), grid, false),
                true,
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 60
                }
            ),
            Err(StrokeError::InvalidContext)
        );
        assert!(
            pending
                .finish(context(clip, beat(8), grid, false), false)
                .unwrap()
                .is_none()
        );

        let mut invalid = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        assert_eq!(
            invalid.update(
                context(clip, beat(8), grid, false),
                false,
                StrokePoint {
                    beat_raw: beat(8),
                    pitch: 60
                }
            ),
            Err(StrokeError::InvalidPoint)
        );
        assert!(
            invalid
                .finish(context(clip, beat(8), grid, false), false)
                .unwrap()
                .is_none()
        );

        let mut invalid_pitch = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        assert_eq!(
            invalid_pitch.update(
                context(clip, beat(8), grid, false),
                false,
                StrokePoint {
                    beat_raw: beat(1),
                    pitch: 128,
                }
            ),
            Err(StrokeError::InvalidPoint)
        );
        assert!(
            invalid_pitch
                .finish(context(clip, beat(8), grid, false), false)
                .unwrap()
                .is_none()
        );

        let mut manually_cancelled = stroke(
            &editor,
            clip,
            beat(8),
            grid,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        manually_cancelled.cancel();
        assert!(
            manually_cancelled
                .finish(context(clip, beat(8), grid, false), false)
                .unwrap()
                .is_none()
        );

        let far_raw = (MAX_STROKE_CELLS as u64 + 1) * MidiGrid::Off.movement_step();
        let long_length = far_raw + beat(2);
        let (long_editor, long_clip) = fixture(long_length);
        let mut overflow = stroke(
            &long_editor,
            long_clip,
            long_length,
            MidiGrid::Off,
            false,
            StrokePoint {
                beat_raw: 0,
                pitch: 60,
            },
        );
        let long_context = context(long_clip, long_length, MidiGrid::Off, false);
        assert_eq!(
            overflow.update(
                long_context,
                false,
                StrokePoint {
                    beat_raw: far_raw,
                    pitch: 60
                }
            ),
            Err(StrokeError::TooManyCells)
        );
        assert!(overflow.finish(long_context, false).unwrap().is_none());

        let too_many_baseline: Arc<[BaselineNote]> = vec![
            BaselineNote {
                id: baseline_id,
                start: 0,
                duration: 1,
                pitch: 60,
                channel: 0,
                velocity: 100,
            };
            MAX_NOTES + 1
        ]
        .into();
        assert_eq!(
            MidiStroke::begin(
                context(clip, beat(8), grid, false),
                beat(8),
                too_many_baseline,
                StrokePoint {
                    beat_raw: 0,
                    pitch: 60,
                },
            )
            .err(),
            Some(StrokeError::TooManyNotes)
        );
    }

    #[test]
    fn blank_view_authoring_is_bounded_and_rejects_overflow_or_malformed_geometry() {
        let (_, clip) = fixture(beat(1));
        let base = context(clip, beat(1), MidiGrid::Sixteenth, false);
        let blank = StrokeContext {
            view_start: beat(8),
            view_span: beat(4),
            ..base
        };
        assert!(blank.is_valid());
        assert_eq!(blank.authoring_length(), Some(beat(12)));
        let distant = StrokeContext {
            view_start: beat(1024),
            ..blank
        };
        assert!(distant.is_valid());
        assert_eq!(distant.authoring_length(), Some(beat(1028)));
        for invalid in [
            StrokeContext {
                view_span: super::super::midi_scene::zoom_limit(blank.clip_length) + 1,
                ..blank
            },
            StrokeContext {
                view_start: u64::MAX,
                view_span: 1,
                ..blank
            },
            StrokeContext {
                view_span: 0,
                ..blank
            },
            StrokeContext {
                clip_length: 0,
                ..blank
            },
            StrokeContext {
                pitch_low: 127,
                pitch_rows: 2,
                ..blank
            },
            StrokeContext {
                bounds: CanvasBounds {
                    width: f32::NAN,
                    ..blank.bounds
                },
                ..blank
            },
        ] {
            assert!(!invalid.is_valid());
        }
    }

    #[test]
    fn every_editing_context_change_cancels_the_stroke() {
        let (mut editor, clip) = fixture(beat(8));
        let Created::Clip(other_clip) = editor
            .execute(Command::PlaceMidiClip {
                track: editor.project().tracks()[0].id(),
                start: BeatTime::from_quarters(8).unwrap(),
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        let base = context(clip, beat(8), MidiGrid::Sixteenth, false);
        let mut generation = base;
        generation.generation += 1;
        let mut revision = base;
        revision.revision += 1;
        let mut changed_clip = base;
        changed_clip.clip = other_clip;
        let mut clip_length = base;
        clip_length.clip_length += 1;
        let mut grid = base;
        grid.grid = MidiGrid::Off;
        let mut view_start = base;
        view_start.view_start += 1;
        let mut view_span = base;
        view_span.view_span -= 1;
        let mut pitch_range = base;
        pitch_range.pitch_low += 1;
        let mut bounds = base;
        bounds.bounds.x += 1.0;
        let mut mode = base;
        mode.draw_mode = false;
        let mut pitch_lock = base;
        pitch_lock.pitch_locked = true;
        let mut snap_modifier = base;
        snap_modifier.snap_modifier = true;
        snap_modifier.effective_grid = MidiGrid::Off;
        let mut pitch_modifier = base;
        pitch_modifier.pitch_modifier = true;
        pitch_modifier.pitch_locked = false;

        for changed in [
            generation,
            revision,
            changed_clip,
            clip_length,
            grid,
            view_start,
            view_span,
            pitch_range,
            bounds,
            mode,
            pitch_lock,
            snap_modifier,
            pitch_modifier,
        ] {
            let mut stroke = MidiStroke::begin(
                base,
                beat(8),
                baseline(&editor, clip),
                StrokePoint {
                    beat_raw: 0,
                    pitch: 60,
                },
            )
            .unwrap();
            assert_eq!(
                stroke.update(
                    changed,
                    false,
                    StrokePoint {
                        beat_raw: beat(1),
                        pitch: 61,
                    }
                ),
                Err(StrokeError::InvalidContext)
            );
            assert!(stroke.finish(base, false).unwrap().is_none());
        }
    }
}
