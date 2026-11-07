//! Revision/viewport-keyed piano-roll geometry and deterministic overlap hits.
//! Time zoom may reveal blank space beyond the source; that space never extends
//! the authored source or clips notes. Pitch rows are part of cache identity and
//! shared by paint, preview and hit geometry.
use hex_project::{ClipId, MidiNote, MidiNoteInsert, NoteId};
use std::{collections::HashMap, sync::Arc};

/// Zoom span stays finite relative to the source. The viewport origin is independent.
pub(super) fn zoom_limit(length: u64) -> u64 {
    length
        .saturating_mul(4)
        .max(hex_project::BeatTime::ONE.raw() * 16)
}

pub(super) fn valid_pitch_range(low: u8, rows: u8) -> bool {
    rows > 0 && u16::from(low) + u16::from(rows) <= 128
}
/// Pitch wheel uses the current rows as one viewport, with a continuous pitch
/// anchored under the pointer. Only integer row alignment and MIDI edges shift it.
/// Each input batch is bounded to four wheel steps, as for horizontal zoom.
pub(super) fn wheel_pitch_zoom(low: u8, rows: u8, fraction: f32, steps: i8) -> Option<(u8, u8)> {
    if !valid_pitch_range(low, rows)
        || !fraction.is_finite()
        || !(0.0..=1.0).contains(&fraction)
        || !(-4..=4).contains(&steps)
    {
        return None;
    }
    let mut next = u16::from(rows);
    for _ in 0..steps.unsigned_abs() {
        next = if steps < 0 {
            (next * 4 / 5).max(1)
        } else {
            (next * 5).div_ceil(4).min(128)
        };
    }
    let anchor = f64::from(low) + (1.0 - f64::from(fraction)) * f64::from(rows);
    let next_low = (anchor - (1.0 - f64::from(fraction)) * f64::from(next))
        .round()
        .clamp(0.0, f64::from(128 - next)) as u8;
    Some((next_low, next as u8))
}

pub(super) const MIN_HIT_WIDTH_PX: f32 = 4.0;
pub(super) fn fit_span(clip_length: u64, beat: u64) -> u64 {
    if clip_length > 0 {
        clip_length
    } else {
        (beat / 16).max(1)
    }
}

pub(super) fn zoom_span(current: u64, clip_length: u64, beat: u64, inward: bool) -> u64 {
    let minimum = (beat / 16).max(1).min(clip_length.max(1));
    let bound = zoom_limit(clip_length).max(minimum);
    let current = current.max(minimum).min(bound);
    if inward {
        (current / 2).max(minimum)
    } else {
        current.saturating_mul(2).min(bound)
    }
}

/// Bounded wheel step in clip-local beat ticks, anchored at the pointer's beat.
/// Integer ratios retain precision at large source coordinates.
pub(super) fn wheel_zoom(
    start: i128,
    span: u64,
    length: u64,
    anchor: i128,
    inward: bool,
) -> (i128, u64) {
    if length == 0 {
        return (0, 0);
    }
    let minimum = (hex_project::BeatTime::ONE.raw() / 16).max(1).min(length);
    let span = span.clamp(minimum, zoom_limit(length));
    let start = clamp_start(start, span);
    let anchor = anchor.clamp(start, start + i128::from(span));
    let next_span = if inward {
        ((u128::from(span) * 4 / 5) as u64).max(minimum)
    } else {
        (u128::from(span) * 5)
            .div_ceil(4)
            .min(u128::from(zoom_limit(length))) as u64
    };
    let offset = ((anchor - start) as u128 * u128::from(next_span) / u128::from(span)) as u64;
    (
        clamp_start(anchor - i128::from(offset), next_span),
        next_span,
    )
}

/// Blank navigation never grows authored data. Only the checked viewport end
/// limits its positive origin; rendering still visits a bounded grid and note set.
pub(super) fn clamp_start(start: i128, span: u64) -> i128 {
    start.clamp(
        -i128::from(u64::MAX),
        i128::from(u64::MAX) - i128::from(span),
    )
}

pub(super) fn pixel_bounds(note: NoteGeometry, width: f32) -> Option<(f32, f32)> {
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    let true_width = note.width * width;
    let hit_width = true_width.max(MIN_HIT_WIDTH_PX).min(width);
    let left = (note.x * width + (true_width - hit_width) / 2.0).max(0.0);
    Some((left, (left + hit_width).min(width)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SceneKey {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) clip: ClipId,
    pub(super) beat_start: i128,
    pub(super) beat_span: u64,
    pub(super) pitch_low: u8,
    pub(super) pitch_rows: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NoteGeometry {
    pub(super) active: bool,
    pub(super) id: NoteId,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) pitch: u8,
    pub(super) channel: u8,
    pub(super) velocity: u8,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) leading_edge_visible: bool,
    pub(super) trailing_edge_visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct InsertGeometry {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HitCycle {
    key: SceneKey,
    row: u8,
    x_cell: u32,
    note: NoteId,
}

/// A passive target shares click/resize geometry without advancing click cycling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NoteHover {
    key: SceneKey,
    pub(super) note: NoteId,
    pub(super) kind: super::midi_drag::HitKind,
}

pub(super) struct MidiScene {
    key: SceneKey,
    notes: Box<[NoteGeometry]>,
    rows: Box<[Box<[usize]>]>,
    by_id: HashMap<NoteId, usize>,
}

#[derive(Default)]
pub(super) struct MidiSceneCache {
    scene: Option<Arc<MidiScene>>,
}

impl MidiScene {
    pub(super) fn build(key: SceneKey, notes: &[MidiNote]) -> Self {
        let mut visible = Vec::with_capacity(notes.len());
        let mut rows = vec![Vec::new(); usize::from(key.pitch_rows)];
        let mut by_id = HashMap::with_capacity(notes.len());
        let view_end = key.beat_start.checked_add(i128::from(key.beat_span));

        if key.beat_span > 0
            && key.beat_start == clamp_start(key.beat_start, key.beat_span)
            && valid_pitch_range(key.pitch_low, key.pitch_rows)
            && let Some(view_end) = view_end
        {
            for note in notes {
                let start = i128::from(note.start().raw());
                let duration = note.duration().raw();
                let Some(end) = start.checked_add(i128::from(duration)) else {
                    continue;
                };
                if end <= key.beat_start
                    || start >= view_end
                    || note.pitch() < key.pitch_low
                    || note.pitch() >= key.pitch_low.saturating_add(key.pitch_rows)
                {
                    continue;
                }
                let row = usize::from(key.pitch_low + (key.pitch_rows - 1) - note.pitch());
                let visible_start = start.max(key.beat_start);
                let visible_end = end.min(view_end);
                let x = (visible_start - key.beat_start) as f64 / key.beat_span as f64;
                let width = (visible_end - visible_start) as f64 / key.beat_span as f64;
                let item = NoteGeometry {
                    id: note.id(),
                    start: note.start().raw(),
                    duration,
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                    active: note.active(),
                    x: x as f32,
                    y: row as f32 / f32::from(key.pitch_rows),
                    width: width as f32,
                    leading_edge_visible: start >= key.beat_start,
                    trailing_edge_visible: end <= view_end,
                };
                let index = visible.len();
                visible.push(item);
                rows[row].push(index);
                by_id.insert(item.id, index);
            }
        }

        Self {
            key,
            notes: visible.into_boxed_slice(),
            rows: rows.into_iter().map(Vec::into_boxed_slice).collect(),
            by_id,
        }
    }

    pub(super) fn key(&self) -> SceneKey {
        self.key
    }

    pub(super) fn notes(&self) -> &[NoteGeometry] {
        &self.notes
    }

    pub(super) fn note(&self, id: NoteId) -> Option<NoteGeometry> {
        self.by_id
            .get(&id)
            .and_then(|index| self.notes.get(*index))
            .copied()
    }

    pub(super) fn hover(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        mut cycle: Option<HitCycle>,
        selected: &[NoteId],
    ) -> Option<NoteHover> {
        let id = self.hit(x, y, width, height, &mut cycle)?;
        let note = self.note(id)?;
        let (left, right) = pixel_bounds(note, width)?;
        Some(NoteHover {
            key: self.key,
            note: id,
            kind: super::midi_drag::selected_hit_kind(
                x,
                left,
                right - left,
                note.leading_edge_visible,
                note.trailing_edge_visible,
                selected.binary_search(&id).is_ok(),
            ),
        })
    }

    pub(super) fn preview_geometry(
        &self,
        id: NoteId,
        start: u64,
        duration: u64,
        pitch: u8,
        channel: u8,
        velocity: u8,
    ) -> Option<NoteGeometry> {
        geometry(self.key, id, start, duration, pitch, channel, velocity)
    }

    /// Render candidate storage coordinates against the same pre-ACK viewport.
    /// Retained canonical notes still use the old scene; the exact terminal moves
    /// the viewport by this same prefix when installing the new canonical source.
    pub(super) fn rebased_preview_geometry(
        &self,
        id: NoteId,
        preview: super::midi_drag::NotePreview,
        prefix: u64,
    ) -> Option<NoteGeometry> {
        let mut key = self.key;
        key.beat_start = key.beat_start.checked_add(i128::from(prefix))?;
        geometry(
            key,
            id,
            preview.start,
            preview.duration,
            preview.pitch,
            0,
            100,
        )
    }

    pub(super) fn insert_geometry(&self, note: MidiNoteInsert) -> Option<InsertGeometry> {
        insert_geometry(self.key, note.start.raw(), note.duration.raw(), note.pitch)
    }

    pub(super) fn hit(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        cycle: &mut Option<HitCycle>,
    ) -> Option<NoteId> {
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || x < 0.0
            || y < 0.0
            || x >= width
            || y >= height
        {
            return None;
        }
        let row = ((y / height * f32::from(self.key.pitch_rows)).floor() as usize)
            .min(self.rows.len().saturating_sub(1));
        let local_x = x;
        let previous = cycle.filter(|last| {
            last.key == self.key && last.row as usize == row && last.x_cell == x.floor() as u32
        });
        let mut first = None;
        let mut after_previous = None;
        let mut previous_found = false;
        let mut passed_previous = false;
        let mut hit_count = 0usize;
        for index in self.rows.get(row)?.iter().rev() {
            let Some(note) = self.notes.get(*index) else {
                continue;
            };
            let Some((left, right)) = pixel_bounds(*note, width) else {
                continue;
            };
            if local_x < left || local_x > right {
                continue;
            }
            first.get_or_insert(note.id);
            hit_count = hit_count.saturating_add(1);
            if passed_previous && after_previous.is_none() {
                after_previous = Some(note.id);
            }
            if previous.is_some_and(|last| last.note == note.id) {
                previous_found = true;
                passed_previous = true;
            }
        }
        let Some(first) = first else {
            *cycle = None;
            return None;
        };
        if hit_count < 2 {
            *cycle = None;
            return Some(first);
        }
        let next = if previous.is_some() && previous_found {
            after_previous.unwrap_or(first)
        } else {
            first
        };
        *cycle = Some(HitCycle {
            key: self.key,
            row: row as u8,
            x_cell: x.floor().clamp(0.0, u32::MAX as f32) as u32,
            note: next,
        });
        Some(next)
    }
}

impl MidiSceneCache {
    pub(super) fn clear(&mut self) {
        self.scene = None;
    }

    pub(super) fn get_or_build(
        &mut self,
        key: SceneKey,
        notes: &[MidiNote],
    ) -> (Arc<MidiScene>, bool) {
        if let Some(scene) = self.scene.as_ref().filter(|scene| scene.key() == key) {
            return (Arc::clone(scene), true);
        }
        let scene = Arc::new(MidiScene::build(key, notes));
        self.scene = Some(Arc::clone(&scene));
        (scene, false)
    }
}

fn geometry(
    key: SceneKey,
    id: NoteId,
    start: u64,
    duration: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
) -> Option<NoteGeometry> {
    let rect = insert_geometry(key, start, duration, pitch)?;
    Some(NoteGeometry {
        id,
        active: true,
        start,
        duration,
        pitch,
        channel,
        velocity,
        x: rect.x,
        y: rect.y,
        width: rect.width,
        leading_edge_visible: i128::from(start) >= key.beat_start,
        trailing_edge_visible: i128::from(start).checked_add(i128::from(duration))?
            <= key.beat_start.checked_add(i128::from(key.beat_span))?,
    })
}

fn insert_geometry(key: SceneKey, start: u64, duration: u64, pitch: u8) -> Option<InsertGeometry> {
    let view_end = key.beat_start.checked_add(i128::from(key.beat_span))?;
    let start = i128::from(start);
    let end = start.checked_add(i128::from(duration))?;
    if key.beat_span == 0
        || key.beat_start != clamp_start(key.beat_start, key.beat_span)
        || !valid_pitch_range(key.pitch_low, key.pitch_rows)
        || duration == 0
        || pitch < key.pitch_low
        || pitch >= key.pitch_low.saturating_add(key.pitch_rows)
        || end <= key.beat_start
        || start >= view_end
    {
        return None;
    }
    let row = usize::from(key.pitch_low + (key.pitch_rows - 1) - pitch);
    let visible_start = start.max(key.beat_start);
    let visible_end = end.min(view_end);
    Some(InsertGeometry {
        x: ((visible_start - key.beat_start) as f64 / key.beat_span as f64) as f32,
        y: row as f32 / f32::from(key.pitch_rows),
        width: ((visible_end - visible_start) as f64 / key.beat_span as f64) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        HitCycle, MIN_HIT_WIDTH_PX, MidiScene, MidiSceneCache, SceneKey, clamp_start, fit_span,
        pixel_bounds, valid_pitch_range, wheel_pitch_zoom, zoom_span,
    };
    use hex_project::{
        BeatTime, Command, Created, Editor, MidiNoteSnapshot, Project, TrackContentSnapshot,
    };
    use std::time::Instant;

    const SPAN: u64 = BeatTime::ONE.raw() * 4;

    fn fixture(count: usize, stagger: bool) -> (Project, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let clip_beats = if count > 600 { 256 } else { 4 };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(8).expect("start"),
                length: BeatTime::from_quarters(clip_beats).expect("length"),
            })
            .expect("clip")
        else {
            panic!("clip result")
        };
        let mut snapshot = editor.project().snapshot();
        let first_note_id = snapshot.next_id;
        let track = snapshot
            .tracks
            .iter_mut()
            .find(|item| item.id == track.value())
            .expect("track snapshot");
        let clips = match &mut track.content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            TrackContentSnapshot::Audio(_) => panic!("track snapshot has no midi lane"),
        };
        let midi_clip = clips.first_mut().expect("clip snapshot");
        let gate = BeatTime::ONE.raw()
            / if stagger && count > 600 {
                16
            } else if !stagger && count > 24 {
                // Adjacent tiny gates have overlapping pixel hit halos.
                4096
            } else {
                8
            };
        midi_clip.notes = (0..count)
            .map(|index| MidiNoteSnapshot {
                active: true,
                id: first_note_id + index as u64,
                start_raw: (index / 24) as u64 * gate
                    + if stagger { 0 } else { BeatTime::ONE.raw() / 4 },
                duration_raw: gate,
                pitch: 48 + (index % 24) as u8,
                channel: 0,
                velocity: 100,
            })
            .collect();
        snapshot.next_id = first_note_id + count as u64;
        (
            Project::from_snapshot(snapshot).expect("project snapshot"),
            clip,
        )
    }

    fn key(clip: hex_project::ClipId) -> SceneKey {
        SceneKey {
            generation: 7,
            revision: 11,
            clip,
            beat_start: 0,
            beat_span: SPAN,
            pitch_low: 48,
            pitch_rows: 24,
        }
    }

    #[test]
    fn scene_includes_and_can_hit_notes_after_the_former_512_note_limit() {
        let (project, clip) = fixture(600, true);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let scene = MidiScene::build(key(clip), notes);
        assert_eq!(scene.notes().len(), 600);
        let late_id = notes[599].id();
        let late = scene.note(late_id).unwrap();
        assert_eq!(late.id, late_id);
        let (left, right) = pixel_bounds(late, 400.0).unwrap();
        let mut cycle = None;
        assert_eq!(
            scene.hit(
                (left + right) / 2.0,
                late.y * 240.0 + 5.0,
                400.0,
                240.0,
                &mut cycle
            ),
            Some(late_id),
            "a note after index 512 must remain hit-testable"
        );
    }

    #[test]
    fn piano_roll_geometry_uses_clip_local_beats_after_late_arrangement_placement() {
        let (project, clip) = fixture(1, false);
        let midi_clip = project.midi_clip(clip).expect("clip");
        assert_eq!(midi_clip.start().raw(), 8 * BeatTime::ONE.raw());
        let note = &midi_clip.notes()[0];
        let scene = MidiScene::build(key(clip), midi_clip.notes());
        let geometry = scene.note(note.id()).expect("visible local note");
        assert_eq!(geometry.start, BeatTime::ONE.raw() / 4);
        assert_eq!(geometry.x, 1.0 / 16.0);
    }

    #[test]
    fn adjacent_same_pitch_hit_halos_cycle_deterministically_and_retain_all_candidates() {
        let (project, clip) = fixture(600, false);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let scene = MidiScene::build(key(clip), notes);
        let mut cycle: Option<HitCycle> = None;
        let pitch = notes[0].pitch();
        let row = f32::from(48 + 23 - pitch) / 24.0;
        let y = row * 240.0 + 1.0;
        let first = scene.hit(26.0, y, 400.0, 240.0, &mut cycle).unwrap();
        let second = scene.hit(26.0, y, 400.0, 240.0, &mut cycle).unwrap();
        assert_ne!(first, second);
        let mut expected: Vec<_> = notes
            .iter()
            .filter(|n| n.pitch() == pitch)
            .map(|n| n.id())
            .collect();
        let mut actual = vec![first, second];
        for _ in 2..expected.len() {
            actual.push(scene.hit(26.0, y, 400.0, 240.0, &mut cycle).unwrap());
        }
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        assert_eq!(scene.hit(26.0, y, 400.0, 240.0, &mut cycle), Some(first));
        assert_eq!(scene.hit(27.0, y, 400.0, 240.0, &mut cycle), Some(first));
    }

    #[test]
    fn cache_reuses_arc_until_generation_revision_clip_or_view_changes() {
        let (project, clip) = fixture(600, true);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let mut cache = MidiSceneCache::default();
        let (first, reused) = cache.get_or_build(key(clip), notes);
        assert!(!reused);
        let (second, reused) = cache.get_or_build(key(clip), notes);
        assert!(reused);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        let original = key(clip);
        for changed in [
            SceneKey {
                generation: original.generation + 1,
                ..original
            },
            SceneKey {
                revision: original.revision + 1,
                ..original
            },
            SceneKey {
                beat_start: i128::from(BeatTime::ONE.raw()),
                ..original
            },
            SceneKey {
                beat_span: SPAN / 2,
                ..original
            },
            SceneKey {
                pitch_low: 49,
                ..original
            },
            SceneKey {
                pitch_rows: 12,
                ..original
            },
        ] {
            let (_, reused) = cache.get_or_build(changed, notes);
            assert!(!reused);
        }
        assert!(!std::sync::Arc::ptr_eq(
            &second,
            &cache.get_or_build(original, notes).0
        ));
    }

    #[test]
    fn signed_navigation_preserves_anchor_and_refuses_unrepresentable_endpoints() {
        let beat = BeatTime::ONE.raw();
        let start = -i128::from(beat * 3 / 2);
        let (next, span) = super::wheel_zoom(start, 4 * beat, 4 * beat, 0, true);
        // The source-zero anchor keeps its normalized pixel, to one Q32 tick.
        assert!(
            ((-next) * i128::from(4 * beat) - (-start) * i128::from(span)).abs()
                <= i128::from(4 * beat)
        );
        assert_eq!(clamp_start(i128::MIN, beat), -i128::from(u64::MAX));
        assert_eq!(clamp_start(i128::MAX, beat), i128::from(u64::MAX - beat));
        let (project, clip) = fixture(1, false);
        let mut k = key(clip);
        k.beat_start = -i128::from(beat);
        k.beat_span = 4 * beat;
        let scene = MidiScene::build(k, project.midi_clip(clip).unwrap().notes());
        let note = scene.notes()[0];
        assert!(
            (note.x - 5.0 / 16.0).abs() < 1e-6,
            "the fixture note is at source beat one quarter"
        );
        k.beat_start = i128::from(u64::MAX);
        assert!(
            MidiScene::build(k, project.midi_clip(clip).unwrap().notes())
                .notes()
                .is_empty()
        );
        assert!(super::insert_geometry(k, 0, beat, 60).is_none());
    }

    #[test]
    fn wheel_zoom_preserves_anchor_and_bounds_at_small_and_large_coordinates() {
        let beat = BeatTime::ONE.raw();
        let (start, span) = super::wheel_zoom(
            i128::from(100 * beat),
            400 * beat,
            2000 * beat,
            i128::from(400 * beat),
            true,
        );
        assert_eq!((start, span), (i128::from(160 * beat), 320 * beat));
        assert_eq!((i128::from(400 * beat) - start) * 4, i128::from(span * 3));
        let mut view = (0, 4 * BeatTime::ONE.raw());
        for _ in 0..256 {
            view = super::wheel_zoom(view.0, view.1, 4 * BeatTime::ONE.raw(), 0, true);
        }
        assert_eq!(view, (0, BeatTime::ONE.raw() / 16));
        for _ in 0..256 {
            view = super::wheel_zoom(view.0, view.1, 4 * BeatTime::ONE.raw(), 0, false);
        }
        assert_eq!(view, (0, super::zoom_limit(4 * BeatTime::ONE.raw())));
        assert_eq!(super::wheel_zoom(0, 1, 1, 1, true), (0, 1));
        assert_eq!(super::wheel_zoom(0, 0, 0, 0, false), (0, 0));
        let view = super::wheel_zoom(
            i128::from(u64::MAX - 400),
            400,
            u64::MAX,
            i128::from(u64::MAX - 100),
            false,
        );
        assert!(view.1 > 0);
        assert!(view.0.checked_add(i128::from(view.1)).is_some());
    }

    #[test]
    fn pitch_span_geometry_and_hits_agree_at_one_and_all_midi_rows() {
        let (project, clip) = fixture(24, false);
        let notes = project.midi_source(clip).unwrap().notes();
        for (low, rows) in [(48, 1), (48, 12), (0, 128)] {
            let scene = MidiScene::build(
                SceneKey {
                    pitch_low: low,
                    pitch_rows: rows,
                    ..key(clip)
                },
                notes,
            );
            for note in scene.notes() {
                let (left, right) = pixel_bounds(*note, 1000.0).unwrap();
                let y = (note.y + 0.5 / f32::from(rows)) * 240.0;
                assert_eq!(
                    super::super::midi_drag::pitch_at_point(
                        y,
                        [0.0, 0.0, 1000.0, 240.0],
                        low,
                        rows
                    ),
                    Some(note.pitch)
                );
                assert_eq!(
                    scene.hit((left + right) / 2.0, y, 1000.0, 240.0, &mut None),
                    Some(note.id)
                );
            }
        }
        assert_eq!(
            super::super::midi_drag::pitch_at_point(f32::MIN, [0.0, 0.0, 100.0, 1.0], 0, 128),
            Some(127)
        );
        assert_eq!(
            super::super::midi_drag::pitch_at_point(f32::MAX, [0.0, 0.0, 100.0, 1.0], 0, 128),
            Some(0)
        );
        for (low, rows) in [(0, 0), (127, 2), (128, 1)] {
            let scene = MidiScene::build(
                SceneKey {
                    pitch_low: low,
                    pitch_rows: rows,
                    ..key(clip)
                },
                notes,
            );
            assert!(scene.notes().is_empty());
            assert!(scene.hit(0.0, 0.0, 1000.0, 240.0, &mut None).is_none());
        }
    }

    #[test]
    fn zoom_span_is_bounded_but_horizontal_origin_only_has_numeric_limit() {
        let beat = BeatTime::ONE.raw();
        let length = 96 * beat;
        assert_eq!(fit_span(length, beat), 96 * beat);
        assert_eq!(zoom_span(4 * beat, length, beat, true), 2 * beat);
        assert_eq!(zoom_span(32 * beat, length, beat, false), 64 * beat);
        assert_eq!(zoom_span(128 * beat, length, beat, false), 256 * beat);
        assert_eq!(fit_span(256 * beat, beat), 256 * beat);
        assert_eq!(zoom_span(beat, length, beat, true), beat / 2);
        assert_eq!(zoom_span(beat / 8, length, beat, true), beat / 16);
        assert_eq!(fit_span(beat / 512, beat), beat / 512);
        assert_eq!(zoom_span(beat / 512, beat / 512, beat, true), beat / 512);
        assert_eq!(
            clamp_start(i128::from(100 * beat), 16 * beat),
            i128::from(100 * beat)
        );
        assert_eq!(
            clamp_start(i128::from(4 * beat), 4 * beat),
            i128::from(4 * beat)
        );
        assert_eq!(
            clamp_start(i128::from(1024 * beat), 4 * beat),
            i128::from(1024 * beat)
        );
        assert_eq!(
            clamp_start(i128::from(u64::MAX), 4 * beat),
            i128::from(u64::MAX - 4 * beat)
        );
        let view = super::wheel_zoom(
            i128::from(u64::MAX - 4 * beat),
            4 * beat,
            beat,
            i128::from(u64::MAX),
            false,
        );
        assert!(view.0.checked_add(i128::from(view.1)).is_some());
        assert!(view.0 > i128::from(1024 * beat));
    }

    #[test]
    fn tiny_note_hit_halo_matches_the_painted_geometry_and_has_no_resize_handle() {
        let (project, clip) = fixture(1, false);
        let midi_clip = project.midi_clip(clip).expect("clip");
        let note = &midi_clip.notes()[0];
        let mut scene_key = key(clip);
        scene_key.beat_span = 256 * BeatTime::ONE.raw();
        let scene = MidiScene::build(scene_key, midi_clip.notes());
        let geometry = scene.note(note.id()).expect("visible note");
        let (left, right) = pixel_bounds(geometry, 400.0).expect("finite width");
        assert_eq!(right - left, MIN_HIT_WIDTH_PX);
        assert_eq!(
            scene.hit(
                left + 1.0,
                geometry.y * 240.0 + 1.0,
                400.0,
                240.0,
                &mut None
            ),
            Some(note.id())
        );
        assert!(!geometry.leading_edge_visible || right - left < 6.0);
        assert_eq!(pixel_bounds(geometry, f32::NAN), None);
    }

    #[test]
    fn geometry_clips_true_edges_and_preview_geometry_keeps_selected_overlay_independent() {
        let (project, clip) = fixture(600, true);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let mut scene_key = key(clip);
        scene_key.beat_start = i128::from(BeatTime::ONE.raw() / 32);
        let scene = MidiScene::build(scene_key, notes);
        let clipped = scene.note(notes[0].id()).unwrap();
        assert!(!clipped.leading_edge_visible);
        let moved = scene
            .preview_geometry(
                clipped.id,
                clipped.start + BeatTime::ONE.raw(),
                clipped.duration,
                clipped.pitch + 1,
                clipped.channel,
                clipped.velocity,
            )
            .unwrap();
        assert_ne!(moved.x, clipped.x);
        assert_eq!(moved.id, clipped.id);
    }

    #[test]
    #[ignore = "run with --release --ignored to profile dense scene build and hit cost"]
    fn benchmark_dense_scene_build_and_hit() {
        for count in [600, 65_536] {
            let (project, clip) = fixture(count, true);
            let midi_clip = project.midi_clip(clip).expect("clip");
            let notes = midi_clip.notes();
            let mut scene_key = key(clip);
            scene_key.beat_span = midi_clip.source_length().raw();
            let start = Instant::now();
            let scene = MidiScene::build(scene_key, notes);
            let build = start.elapsed();
            let mut cycle = None;
            let start = Instant::now();
            for _ in 0..100 {
                std::hint::black_box(scene.hit(125.0, 120.0, 400.0, 240.0, &mut cycle));
            }
            println!(
                "notes={count} visible={} build_us={:.1} 100_hits_us={:.1}",
                scene.notes().len(),
                build.as_secs_f64() * 1_000_000.0,
                start.elapsed().as_secs_f64() * 1_000_000.0
            );
        }
    }
    #[test]
    fn pitch_wheel_zoom_keeps_valid_rows_and_pointer_anchor_at_pitch_edges() {
        for (low, rows) in [
            (0, 1),
            (127, 1),
            (0, 128),
            (48, 24),
            (0, 12),
            (116, 12),
            (60, 1),
            (40, 64),
        ] {
            for fraction in [0.0, 0.5, 1.0] {
                for steps in [-4, -1, 0, 1, 4] {
                    let (next_low, next_rows) =
                        wheel_pitch_zoom(low, rows, fraction, steps).unwrap();
                    assert!(valid_pitch_range(next_low, next_rows));
                    if steps < 0 {
                        assert!(next_rows <= rows);
                    } else {
                        assert!(next_rows >= rows);
                    }
                    let anchor = f32::from(low) + (1.0 - fraction) * f32::from(rows);
                    let actual = f32::from(next_low) + (1.0 - fraction) * f32::from(next_rows);
                    if next_low > 0 && u16::from(next_low) + u16::from(next_rows) < 128 {
                        assert!((actual - anchor).abs() <= 0.5);
                    }
                }
            }
        }
        for (low, rows, fraction, steps) in [
            (0, 0, 0.5, 1),
            (127, 2, 0.5, 1),
            (128, 1, 0.5, 1),
            (0, 24, f32::NAN, 1),
            (0, 24, -0.1, 1),
            (0, 24, 1.1, 1),
            (0, 24, 0.5, i8::MIN),
            (0, 24, 0.5, 5),
        ] {
            assert_eq!(wheel_pitch_zoom(low, rows, fraction, steps), None);
        }
    }
}
