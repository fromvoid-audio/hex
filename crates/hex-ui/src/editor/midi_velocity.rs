//! Time-aligned velocity geometry and cancellable, single-command drag payloads.
//! Pitch visibility never filters this lane. Canonical notes/history stay in
//! project/session; previews retain immutable originals and never edit playback.
use super::{midi_group_drag::GroupNote, midi_stroke::CanvasBounds};
use hex_project::{BeatTime, ClipId, MidiNote, MidiNoteUpdate, NoteId};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SceneKey {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) start: i128,
    pub(super) span: u64,
}
impl SceneKey {
    fn end(self) -> Option<i128> {
        (self.span > 0 && self.start == super::midi_scene::clamp_start(self.start, self.span))
            .then_some(())?;
        self.start.checked_add(i128::from(self.span))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub(super) note: GroupNote,
    pub(super) x: f32,
}
pub(super) struct Scene {
    pub(super) key: SceneKey,
    points: Box<[Point]>,
}
impl Scene {
    pub(super) fn build(key: SceneKey, notes: &[MidiNote]) -> Result<Self, &'static str> {
        let end = key.end().ok_or("invalid velocity viewport")?;
        if notes.len() > hex_project::MAX_NOTES {
            return Err("velocity lane exceeds the note limit");
        }
        let visible = |note: &&MidiNote| (key.start..end).contains(&i128::from(note.start().raw()));
        let mut points = Vec::new();
        points
            .try_reserve_exact(notes.iter().filter(visible).count())
            .map_err(|_| "could not reserve velocity lane")?;
        for note in notes.iter().filter(visible) {
            points.push(Point {
                note: GroupNote {
                    id: note.id(),
                    active: note.active(),
                    start: note.start().raw(),
                    duration: note.duration().raw(),
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                },
                x: ((i128::from(note.start().raw()) - key.start) as f64 / key.span as f64) as f32,
            });
        }
        points.sort_unstable_by_key(|point| (point.note.start, point.note.id));
        Ok(Self {
            key,
            points: points.into_boxed_slice(),
        })
    }
    pub(super) fn points(&self) -> &[Point] {
        &self.points
    }
    /// Rectangle intersections use the actual handle and stem extents, including
    /// notes outside the piano's pitch view. Results are canonical note identities.
    pub(super) fn marquee_hits(
        &self,
        first: [f32; 2],
        last: [f32; 2],
        width: f32,
        height: f32,
    ) -> Result<Vec<NoteId>, &'static str> {
        if !first
            .into_iter()
            .chain(last)
            .chain([width, height])
            .all(f32::is_finite)
            || width <= 0.0
            || height <= 8.0
        {
            return Err("invalid velocity selection geometry");
        }
        let [left, top, right, bottom] = [
            first[0].min(last[0]),
            first[1].min(last[1]),
            first[0].max(last[0]),
            first[1].max(last[1]),
        ];
        let mut hits = Vec::new();
        hits.try_reserve(self.points.len())
            .map_err(|_| "could not reserve velocity selection")?;
        for point in self.points.iter() {
            let x = point.x * width;
            let Some(y) = handle_y(point.note.velocity, height) else {
                continue;
            };
            let handle = x + 3.0 >= left && x - 3.0 <= right && y + 2.0 >= top && y - 2.0 <= bottom;
            let stem = x + 1.0 >= left && x <= right && height - 4.0 >= top && y <= bottom;
            if handle || stem {
                hits.push(point.note.id);
            }
        }
        hits.sort_unstable();
        hits.dedup();
        Ok(hits)
    }
    /// Pick the nearest handle. Equal-position handles cycle by stable note ID;
    /// the caller retains `after` only for repeated clicks at the same location.
    pub(super) fn hit(
        &self,
        position: [f32; 2],
        size: [f32; 2],
        after: Option<NoteId>,
    ) -> Option<NoteId> {
        let [x, y] = position;
        let [width, height] = size;
        if ![x, y, width, height].into_iter().all(f32::is_finite)
            || width <= 0.0
            || height <= 0.0
            || x < 0.0
            || x >= width
            || y < 0.0
            || y >= height
        {
            return None;
        }
        let low = self
            .points
            .partition_point(|point| point.x * width < x - 6.0);
        let mut best_distance = f32::INFINITY;
        let mut first = None;
        let mut next = None;
        for point in self.points[low..]
            .iter()
            .take_while(|point| point.x * width <= x + 6.0)
        {
            let dy = handle_y(point.note.velocity, height)? - y;
            let dx = point.x * width - x;
            let distance = dx * dx + dy * dy;
            let on_stem =
                dx.abs() <= 3.0 && y >= handle_y(point.note.velocity, height)? && y <= height - 4.0;
            if distance > 36.0 && !on_stem {
                continue;
            }
            if distance < best_distance - 0.01 {
                best_distance = distance;
                first = Some(point.note.id);
                next = after
                    .filter(|after| point.note.id > *after)
                    .map(|_| point.note.id);
            } else if (distance - best_distance).abs() <= 0.01 {
                first = Some(first.map_or(point.note.id, |id| id.min(point.note.id)));
                if after.is_some_and(|after| point.note.id > after) {
                    next = Some(next.map_or(point.note.id, |id| id.min(point.note.id)));
                }
            }
        }
        next.or(first)
    }
}
/// A small inset keeps minimum/maximum handles inside the operable lane.
pub(super) fn handle_y(velocity: u8, height: f32) -> Option<f32> {
    if !(1..=127).contains(&velocity) || !height.is_finite() || height <= 8.0 {
        return None;
    }
    Some(4.0 + (127.0 - f32::from(velocity)) / 126.0 * (height - 8.0))
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Context {
    pub(super) scene: SceneKey,
    pub(super) bounds: CanvasBounds,
    pub(super) source_length: u64,
    pub(super) selection_epoch: u64,
}
impl Context {
    fn valid(self) -> bool {
        self.scene.end().is_some()
            && self.bounds.is_valid()
            && self.bounds.height > 8.0
            && self.source_length > 0
    }
}
#[derive(Clone)]
pub(super) struct Gesture {
    context: Context,
    originals: Arc<[GroupNote]>,
    start_y: f32,
    delta: i16,
}
impl Gesture {
    pub(super) fn begin(
        context: Context,
        mut originals: Vec<GroupNote>,
        start_y: f32,
    ) -> Result<Self, &'static str> {
        if !context.valid()
            || !start_y.is_finite()
            || originals.is_empty()
            || originals.len() > hex_project::MAX_NOTES
        {
            return Err("invalid velocity gesture");
        }
        originals.sort_unstable_by_key(|note| note.id);
        if originals.windows(2).any(|notes| notes[0].id == notes[1].id)
            || originals.iter().any(|note| {
                note.pitch > 127
                    || note.channel > 15
                    || !(1..=127).contains(&note.velocity)
                    || note.duration == 0
                    || note
                        .start
                        .checked_add(note.duration)
                        .is_none_or(|end| end > context.source_length)
            })
        {
            return Err("velocity originals changed · edit canceled");
        }
        Ok(Self {
            context,
            originals: originals.into(),
            start_y,
            delta: 0,
        })
    }
    /// Copied immutable admission identity; geometry updates never replace it.
    pub(super) fn admission(&self) -> (Context, f32) {
        (self.context, self.start_y)
    }

    pub(super) fn is_current(&self, context: Context) -> bool {
        self.context == context && context.valid()
    }
    pub(super) fn update(&mut self, context: Context, y: f32) -> Result<bool, &'static str> {
        if !self.is_current(context) || !y.is_finite() {
            return Err("velocity context changed · edit canceled");
        }
        let delta = (((f64::from(self.start_y) - f64::from(y))
            / f64::from(context.bounds.height - 8.0))
            * 126.0)
            .round()
            .clamp(-126.0, 126.0) as i16;
        let changed = delta != self.delta;
        self.delta = delta;
        Ok(changed)
    }
    pub(super) fn preview(&self, id: NoteId) -> Option<u8> {
        let index = self
            .originals
            .binary_search_by_key(&id, |note| note.id)
            .ok()?;
        Some(self.velocity(self.originals.get(index)?.velocity))
    }
    fn velocity(&self, original: u8) -> u8 {
        (i16::from(original) + self.delta).clamp(1, 127) as u8
    }
    pub(super) fn updates(
        &self,
        context: Context,
    ) -> Result<Option<Vec<MidiNoteUpdate>>, &'static str> {
        if !self.is_current(context) {
            return Err("velocity context changed · edit canceled");
        }
        if !self
            .originals
            .iter()
            .any(|note| self.velocity(note.velocity) != note.velocity)
        {
            return Ok(None);
        }
        let mut updates = Vec::new();
        updates
            .try_reserve_exact(self.originals.len())
            .map_err(|_| "could not reserve velocity edit")?;
        updates.extend(self.originals.iter().map(|note| MidiNoteUpdate {
            note: note.id,
            start: BeatTime::from_raw(note.start),
            duration: BeatTime::from_raw(note.duration),
            pitch: note.pitch,
            channel: note.channel,
            velocity: self.velocity(note.velocity),
        }));
        Ok(Some(updates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    fn fixture() -> (Editor, ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "velocity".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for (pitch, start, velocity) in [(0, 1, 64), (127, 1, 64), (60, 3, 37)] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_quarters(start).unwrap(),
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                    pitch,
                    channel: 2,
                    velocity,
                })
                .unwrap();
        }
        (editor, clip)
    }
    fn key(clip: ClipId) -> SceneKey {
        SceneKey {
            generation: 7,
            revision: 11,
            clip,
            start: 0,
            span: BeatTime::ONE.raw() * 2,
        }
    }
    fn context(clip: ClipId) -> Context {
        Context {
            scene: key(clip),
            bounds: CanvasBounds {
                x: 30.0,
                y: 20.0,
                width: 400.0,
                height: 64.0,
            },
            source_length: BeatTime::ONE.raw() * 4,
            selection_epoch: 4,
        }
    }
    #[test]
    fn marquee_intersects_stems_and_coincident_hidden_pitch_handles_in_both_directions() {
        let (editor, clip) = fixture();
        let scene =
            Scene::build(key(clip), editor.project().midi_clip(clip).unwrap().notes()).unwrap();
        let ids = scene.points().iter().map(|p| p.note.id).collect::<Vec<_>>();
        for (first, last) in [
            ([195.0, 50.0], [205.0, 59.0]),
            ([205.0, 59.0], [195.0, 50.0]),
        ] {
            assert_eq!(scene.marquee_hits(first, last, 400.0, 64.0).unwrap(), ids);
        }
        assert_eq!(scene.hit([200.0, 57.0], [400.0, 64.0], None), Some(ids[0]));
        assert_eq!(
            scene.hit([200.0, 57.0], [400.0, 64.0], Some(ids[0])),
            Some(ids[1])
        );
        assert!(
            scene
                .marquee_hits([0.0, 0.0], [190.0, 64.0], 400.0, 64.0)
                .unwrap()
                .is_empty()
        );
        assert!(
            scene
                .marquee_hits([f32::NAN, 0.0], [205.0, 59.0], 400.0, 64.0)
                .is_err()
        );
    }
    #[test]
    fn lane_uses_onsets_across_all_pitches_and_cycles_coincident_handles() {
        let (editor, clip) = fixture();
        let source = editor.project().midi_source(clip).unwrap();
        let scene = Scene::build(key(clip), source.notes()).unwrap();
        assert_eq!(
            scene
                .points()
                .iter()
                .map(|p| p.note.pitch)
                .collect::<Vec<_>>(),
            [0, 127]
        );
        let y = handle_y(64, 64.0).unwrap();
        let first = scene.hit([200.0, y], [400.0, 64.0], None).unwrap();
        let second = scene.hit([200.0, y], [400.0, 64.0], Some(first)).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            scene.hit([200.0, y], [400.0, 64.0], Some(second)),
            Some(first)
        );
        assert_eq!(scene.hit([210.0, y], [400.0, 64.0], None), None);
        let mut zoomed = key(clip);
        zoomed.start = i128::from(BeatTime::ONE.raw());
        zoomed.span = BeatTime::ONE.raw();
        let scene = Scene::build(zoomed, source.notes()).unwrap();
        assert!(scene.points().iter().all(|point| point.x == 0.0));
        zoomed.start += i128::from(BeatTime::ONE.raw() / 2);
        assert!(
            Scene::build(zoomed, source.notes())
                .unwrap()
                .points()
                .is_empty(),
            "no fake onset at the clipped viewport edge"
        );
    }
    #[test]
    fn velocity_group_drag_is_one_canonical_history_action_and_keeps_other_fields() {
        let (mut editor, clip) = fixture();
        let before = editor.project().snapshot();
        let mut viewport = key(clip);
        viewport.span = BeatTime::ONE.raw() * 4;
        let scene = Scene::build(
            viewport,
            editor.project().midi_source(clip).unwrap().notes(),
        )
        .unwrap();
        let originals: Vec<_> = scene.points().iter().map(|point| point.note).collect();
        let mut context = context(clip);
        context.scene = viewport;
        let mut gesture = Gesture::begin(context, originals.clone(), 32.0).unwrap();
        assert!(gesture.updates(context).unwrap().is_none());
        gesture.update(context, 18.0).unwrap();
        let updates = gesture.updates(context).unwrap().unwrap();
        for (original, update) in originals.iter().zip(&updates) {
            assert_eq!(
                (
                    update.note,
                    update.start.raw(),
                    update.duration.raw(),
                    update.pitch,
                    update.channel
                ),
                (
                    original.id,
                    original.start,
                    original.duration,
                    original.pitch,
                    original.channel
                )
            );
            assert_eq!(update.velocity, original.velocity + 32);
            assert_eq!(gesture.preview(original.id), Some(update.velocity));
        }
        editor
            .execute(Command::UpdateMidiNotes {
                clip,
                notes: updates,
            })
            .unwrap();
        let after = editor.project().snapshot();
        assert_ne!(before, after);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        gesture.update(context, -10_000.0).unwrap();
        assert!(
            gesture
                .updates(context)
                .unwrap()
                .unwrap()
                .iter()
                .all(|note| note.velocity == 127)
        );
        gesture.update(context, 10_000.0).unwrap();
        assert!(
            gesture
                .updates(context)
                .unwrap()
                .unwrap()
                .iter()
                .all(|note| note.velocity == 1)
        );
    }
    #[test]
    fn invalid_geometry_and_changed_context_cannot_commit_a_preview() {
        let (editor, clip) = fixture();
        let scene = Scene::build(
            key(clip),
            editor.project().midi_source(clip).unwrap().notes(),
        )
        .unwrap();
        let originals: Vec<_> = scene.points().iter().map(|point| point.note).collect();
        let original_context = context(clip);
        let mut gesture = Gesture::begin(original_context, originals, 32.0).unwrap();
        let mut stale = original_context;
        stale.scene.revision += 1;
        assert!(gesture.update(stale, 12.0).is_err());
        stale = original_context;
        stale.selection_epoch += 1;
        assert!(gesture.update(stale, 12.0).is_err());
        stale = original_context;
        stale.bounds.width += 1.0;
        assert!(gesture.update(stale, 12.0).is_err());
        assert!(gesture.updates(stale).is_err());
        assert!(gesture.update(original_context, f32::NAN).is_err());
        assert!(gesture.updates(original_context).unwrap().is_none());
        let mut invalid = key(clip);
        invalid.span = 0;
        assert!(Scene::build(invalid, &[]).is_err());
        assert!(scene.hit([f32::NAN, 0.0], [400.0, 64.0], None).is_none());
        assert!(handle_y(0, 64.0).is_none());
    }
}
