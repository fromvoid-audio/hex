//! Revision-keyed complete piano-roll geometry and deterministic overlap hits.
use hex_project::{ClipId, MidiNote, MidiNoteInsert, NoteId};
use std::{collections::HashMap, sync::Arc};

pub(super) const ROWS: usize = 24;
pub(super) const MIN_HIT_WIDTH_PX: f32 = 4.0;
pub(super) fn fit_span(clip_length: u64, beat: u64) -> u64 {
    if clip_length > 0 {
        clip_length
    } else {
        (beat / 16).max(1)
    }
}

pub(super) fn zoom_span(current: u64, clip_length: u64, beat: u64, inward: bool) -> u64 {
    let minimum = (beat / 16).max(1);
    if clip_length > 0 && clip_length < minimum {
        return clip_length;
    }
    let bound = clip_length.max(minimum);
    let current = current.max(minimum).min(bound);
    if inward {
        (current / 2).max(minimum)
    } else {
        current.saturating_mul(2).min(bound)
    }
}

pub(super) fn clamp_start(start: u64, clip_length: u64, span: u64) -> u64 {
    start.min(clip_length.saturating_sub(span))
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
    pub(super) beat_start: u64,
    pub(super) beat_span: u64,
    pub(super) pitch_low: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NoteGeometry {
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

pub(super) struct MidiScene {
    key: SceneKey,
    notes: Box<[NoteGeometry]>,
    rows: [Box<[usize]>; ROWS],
    by_id: HashMap<NoteId, usize>,
}

#[derive(Default)]
pub(super) struct MidiSceneCache {
    scene: Option<Arc<MidiScene>>,
}

impl MidiScene {
    pub(super) fn build(key: SceneKey, notes: &[MidiNote]) -> Self {
        let mut visible = Vec::with_capacity(notes.len());
        let mut rows: [Vec<usize>; ROWS] = std::array::from_fn(|_| Vec::new());
        let mut by_id = HashMap::with_capacity(notes.len());
        let view_end = key.beat_start.checked_add(key.beat_span);

        if key.beat_span > 0
            && key.pitch_low <= 104
            && let Some(view_end) = view_end
        {
            for note in notes {
                let start = note.start().raw();
                let duration = note.duration().raw();
                let Some(end) = start.checked_add(duration) else {
                    continue;
                };
                if end <= key.beat_start
                    || start >= view_end
                    || note.pitch() < key.pitch_low
                    || note.pitch() >= key.pitch_low.saturating_add(ROWS as u8)
                {
                    continue;
                }
                let row = usize::from(key.pitch_low + 23 - note.pitch());
                let visible_start = start.max(key.beat_start);
                let visible_end = end.min(view_end);
                let x = (visible_start - key.beat_start) as f64 / key.beat_span as f64;
                let width = (visible_end - visible_start) as f64 / key.beat_span as f64;
                let item = NoteGeometry {
                    id: note.id(),
                    start,
                    duration,
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                    x: x as f32,
                    y: row as f32 / ROWS as f32,
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
            rows: rows.map(Vec::into_boxed_slice),
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
        let row = ((y / height * ROWS as f32).floor() as usize).min(ROWS - 1);
        let local_x = x;
        let previous = cycle.filter(|last| {
            last.key == self.key && last.row as usize == row && last.x_cell == x.floor() as u32
        });
        let mut first = None;
        let mut after_previous = None;
        let mut previous_found = false;
        let mut passed_previous = false;
        let mut hit_count = 0usize;
        for index in self.rows[row].iter().rev() {
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
        start,
        duration,
        pitch,
        channel,
        velocity,
        x: rect.x,
        y: rect.y,
        width: rect.width,
        leading_edge_visible: start >= key.beat_start,
        trailing_edge_visible: start.checked_add(duration)?
            <= key.beat_start.checked_add(key.beat_span)?,
    })
}

fn insert_geometry(key: SceneKey, start: u64, duration: u64, pitch: u8) -> Option<InsertGeometry> {
    let view_end = key.beat_start.checked_add(key.beat_span)?;
    let end = start.checked_add(duration)?;
    if key.beat_span == 0
        || key.pitch_low > 104
        || duration == 0
        || pitch < key.pitch_low
        || pitch >= key.pitch_low.saturating_add(ROWS as u8)
        || end <= key.beat_start
        || start >= view_end
    {
        return None;
    }
    let row = usize::from(key.pitch_low + 23 - pitch);
    let visible_start = start.max(key.beat_start);
    let visible_end = end.min(view_end);
    Some(InsertGeometry {
        x: ((visible_start - key.beat_start) as f64 / key.beat_span as f64) as f32,
        y: row as f32 / ROWS as f32,
        width: ((visible_end - visible_start) as f64 / key.beat_span as f64) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        HitCycle, MIN_HIT_WIDTH_PX, MidiScene, MidiSceneCache, SceneKey, clamp_start, fit_span,
        pixel_bounds, zoom_span,
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
        midi_clip.notes = (0..count)
            .map(|index| MidiNoteSnapshot {
                id: 3 + index as u64,
                start_raw: if stagger {
                    ((index
                        % if count > 600 {
                            clip_beats as usize * 16 - 4
                        } else {
                            16
                        }) as u64)
                        * BeatTime::ONE.raw()
                        / 16
                } else {
                    BeatTime::ONE.raw() / 4
                },
                duration_raw: if stagger {
                    BeatTime::ONE.raw() / 4
                } else {
                    BeatTime::ONE.raw() / 8
                },
                pitch: 48 + (index % 24) as u8,
                channel: 0,
                velocity: 100,
            })
            .collect();
        snapshot.next_id = 3 + count as u64;
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
        }
    }

    #[test]
    fn scene_includes_and_can_hit_notes_after_the_former_512_note_limit() {
        let (project, clip) = fixture(600, true);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let scene = MidiScene::build(key(clip), notes);
        assert_eq!(scene.notes().len(), 600);
        let late_id = notes[599].id();
        assert_eq!(scene.note(late_id).unwrap().id, late_id);
        let mut cycle = None;
        let mut found_late = false;
        for _ in 0..64 {
            found_late |= scene.hit(56.0, 5.0, 400.0, 240.0, &mut cycle) == Some(late_id);
        }
        assert!(
            found_late,
            "every overlapping late note must remain hit-testable"
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
    fn same_pitch_overlaps_cycle_deterministically_and_retain_all_candidates() {
        let (project, clip) = fixture(600, false);
        let notes = project.midi_clip(clip).expect("clip").notes();
        let scene = MidiScene::build(key(clip), notes);
        let mut cycle: Option<HitCycle> = None;
        let pitch = notes[0].pitch();
        let row = f32::from(48 + 23 - pitch) / 24.0;
        let y = row * 240.0 + 1.0;
        let first = scene.hit(30.0, y, 400.0, 240.0, &mut cycle).unwrap();
        let second = scene.hit(30.0, y, 400.0, 240.0, &mut cycle).unwrap();
        assert_ne!(first, second);
        assert_eq!(scene.hit(31.0, y, 400.0, 240.0, &mut cycle), Some(first));
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
                beat_start: BeatTime::ONE.raw(),
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
    fn horizontal_zoom_and_pan_stay_inside_local_clip_bounds() {
        let beat = BeatTime::ONE.raw();
        let length = 96 * beat;
        assert_eq!(fit_span(length, beat), 96 * beat);
        assert_eq!(zoom_span(4 * beat, length, beat, true), 2 * beat);
        assert_eq!(zoom_span(32 * beat, length, beat, false), 64 * beat);
        assert_eq!(zoom_span(128 * beat, length, beat, false), 96 * beat);
        assert_eq!(fit_span(256 * beat, beat), 256 * beat);
        assert_eq!(zoom_span(beat, length, beat, true), beat / 2);
        assert_eq!(zoom_span(beat / 8, length, beat, true), beat / 16);
        assert_eq!(fit_span(beat / 512, beat), beat / 512);
        assert_eq!(zoom_span(beat / 512, beat / 512, beat, true), beat / 512);
        assert_eq!(clamp_start(100 * beat, length, 16 * beat), 80 * beat);
        assert_eq!(clamp_start(4 * beat, 3 * beat, 4 * beat), 0);
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
        scene_key.beat_start = BeatTime::ONE.raw() / 32;
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
}
