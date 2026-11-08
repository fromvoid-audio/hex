//! Bounded piano-roll note selection and marquee geometry.
use super::midi_scene::{self, NoteGeometry};
use hex_project::NoteId;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MarqueeContext {
    pub(super) scene: midi_scene::SceneKey,
    pub(super) bounds: super::midi_stroke::CanvasBounds,
    pub(super) selection_epoch: u64,
}

/// Cancellable live selection, retaining an immutable baseline for additive hits and rollback.
/// Context identity includes geometry and selection, so a stale preview cannot win.
#[derive(Clone)]
pub(super) struct Marquee {
    first: [f32; 2],
    last: [f32; 2],
    context: MarqueeContext,
    additive: bool,
    baseline: MidiSelection,
    hits: Arc<[NoteId]>,
    highlighted: Arc<[NoteId]>,
}

impl Marquee {
    pub(super) fn begin(
        context: MarqueeContext,
        first: [f32; 2],
        additive: bool,
        selection: &MidiSelection,
    ) -> Option<Self> {
        if !context.bounds.is_valid()
            || context.scene.beat_span == 0
            || context.scene.beat_start
                != midi_scene::clamp_start(context.scene.beat_start, context.scene.beat_span)
            || context
                .scene
                .beat_start
                .checked_add(i128::from(context.scene.beat_span))
                .is_none()
            || !midi_scene::valid_pitch_range(context.scene.pitch_low, context.scene.pitch_rows)
            || !first.into_iter().all(f32::is_finite)
            || first[0] < 0.0
            || first[0] >= context.bounds.width
            || first[1] < 0.0
            || first[1] >= context.bounds.height
            || context.selection_epoch != selection.epoch()
        {
            return None;
        }
        Some(Self {
            first,
            last: first,
            context,
            additive,
            baseline: selection.clone(),
            hits: Arc::from([]),
            highlighted: if additive {
                selection.shared_ids()
            } else {
                Arc::from([])
            },
        })
    }

    pub(super) fn is_current(&self, context: MarqueeContext) -> bool {
        self.context == context && context.bounds.is_valid()
    }

    pub(super) fn highlighted(&self) -> Arc<[NoteId]> {
        Arc::clone(&self.highlighted)
    }

    pub(super) fn rectangle(&self) -> ([f32; 2], [f32; 2]) {
        (self.first, self.last)
    }

    pub(super) fn update(
        &mut self,
        context: MarqueeContext,
        selection: &MidiSelection,
        scene: &midi_scene::MidiScene,
        last: [f32; 2],
    ) -> Result<bool, &'static str> {
        if scene.key() != context.scene {
            return Err("midi marquee changed · selection canceled");
        }
        self.update_with_hits(context, selection, last, |first, last| {
            marquee_hits(
                scene.notes(),
                first,
                last,
                context.bounds.width,
                context.bounds.height,
                context.scene.pitch_rows,
            )
        })
    }

    pub(super) fn context(&self) -> MarqueeContext {
        self.context
    }

    pub(super) fn additive(&self) -> bool {
        self.additive
    }

    /// Surfaces supply their own bounded hit geometry; selection, additive preview,
    /// epoch validation and release stay with this single selection owner.
    /// The supplier returns sorted, unique canonical note IDs.
    pub(super) fn update_with_hits(
        &mut self,
        context: MarqueeContext,
        selection: &MidiSelection,
        last: [f32; 2],
        hits: impl FnOnce([f32; 2], [f32; 2]) -> Result<Vec<NoteId>, &'static str>,
    ) -> Result<bool, &'static str> {
        if !self.is_current(context)
            || selection.epoch() != context.selection_epoch
            || !last.into_iter().all(f32::is_finite)
        {
            return Err("midi marquee changed · selection canceled");
        }
        let changed = self.last != last;
        self.last = last;
        let hits = if self.has_range() {
            hits(self.first, last)?
        } else {
            Vec::new()
        };
        if self.hits.as_ref() != hits.as_slice() {
            // Reuse immutable paint data while motion stays over the same hit set.
            let hits: Arc<[NoteId]> = Arc::from(hits);
            let highlighted = if self.additive {
                let mut preview = self.baseline.clone();
                preview.add_many(&hits)?;
                preview.shared_ids()
            } else {
                Arc::clone(&hits)
            };
            self.hits = hits;
            self.highlighted = highlighted;
        }
        Ok(changed)
    }

    pub(super) fn hits(&self) -> Arc<[NoteId]> {
        Arc::clone(&self.hits)
    }

    pub(super) fn apply_live(
        &mut self,
        selection: &mut MidiSelection,
    ) -> Result<bool, &'static str> {
        if selection.epoch() != self.context.selection_epoch {
            return Err("midi marquee selection changed · capture canceled");
        }
        let primary = if self.additive {
            self.baseline
                .primary
                .or_else(|| self.highlighted.last().copied())
        } else {
            self.highlighted.first().copied()
        };
        let changed = selection.replace_shared(Arc::clone(&self.highlighted), primary);
        self.context.selection_epoch = selection.epoch();
        Ok(changed)
    }

    pub(super) fn restore_baseline(self, selection: &mut MidiSelection) -> bool {
        if selection.epoch() != self.context.selection_epoch {
            return false;
        }
        selection.replace_shared(self.baseline.ids, self.baseline.primary)
    }

    fn has_range(&self) -> bool {
        (self.first[0] - self.last[0]).abs() >= 3.0 || (self.first[1] - self.last[1]).abs() >= 3.0
    }
}

#[derive(Clone)]
pub(super) struct MidiSelection {
    ids: Arc<[NoteId]>,
    primary: Option<NoteId>,
    epoch: u64,
}

impl Default for MidiSelection {
    fn default() -> Self {
        Self {
            ids: Arc::from([]),
            primary: None,
            epoch: 0,
        }
    }
}

impl MidiSelection {
    pub(super) fn ids(&self) -> &[NoteId] {
        &self.ids
    }

    pub(super) fn shared_ids(&self) -> Arc<[NoteId]> {
        Arc::clone(&self.ids)
    }

    pub(super) fn primary(&self) -> Option<NoteId> {
        self.primary
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(super) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(super) fn contains(&self, id: NoteId) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    pub(super) fn set_single(&mut self, id: NoteId) -> bool {
        if self.ids.as_ref() == [id] && self.primary == Some(id) {
            return false;
        }
        self.ids = Arc::from([id]);
        self.primary = Some(id);
        self.bump_epoch();
        true
    }

    pub(super) fn clear(&mut self) -> bool {
        if self.ids.is_empty() && self.primary.is_none() {
            return false;
        }
        self.ids = Arc::from([]);
        self.primary = None;
        self.bump_epoch();
        true
    }

    pub(super) fn toggle(&mut self, id: NoteId) -> Result<bool, &'static str> {
        let mut next = Vec::new();
        next.try_reserve_exact(self.ids.len().saturating_add(1))
            .map_err(|_| "could not reserve midi selection")?;
        next.extend_from_slice(&self.ids);
        let primary = match next.binary_search(&id) {
            Ok(index) => {
                next.remove(index);
                if self.primary == Some(id) {
                    next.last().copied()
                } else {
                    self.primary
                }
            }
            Err(index) => {
                next.insert(index, id);
                Some(id)
            }
        };
        self.replace(next, primary)
    }

    /// Add marquee hits to the immutable selection baseline.
    pub(super) fn add_many(&mut self, ids: &[NoteId]) -> Result<bool, &'static str> {
        if ids.is_empty() {
            return Ok(false);
        }
        let mut next = Vec::new();
        next.try_reserve_exact(self.ids.len().saturating_add(ids.len()))
            .map_err(|_| "could not reserve midi marquee selection")?;
        next.extend_from_slice(&self.ids);
        next.extend_from_slice(ids);
        next.sort_unstable();
        next.dedup();
        let primary = self.primary.or_else(|| next.last().copied());
        self.replace(next, primary)
    }

    pub(super) fn select_all(
        &mut self,
        ids: impl IntoIterator<Item = NoteId>,
    ) -> Result<bool, &'static str> {
        let mut next = Vec::new();
        for id in ids {
            if next.len() >= hex_project::MAX_NOTES {
                return Err("midi selection exceeds note limit");
            }
            next.try_reserve(1)
                .map_err(|_| "could not reserve select-all midi notes")?;
            next.push(id);
        }
        next.sort_unstable();
        next.dedup();
        let primary = next.first().copied();
        self.replace(next, primary)
    }

    /// Canonical creation selects its complete authored set, with the returned last ID primary.
    pub(super) fn select_created(
        &mut self,
        mut ids: Vec<NoteId>,
        primary: NoteId,
    ) -> Result<bool, &'static str> {
        ids.sort_unstable();
        ids.dedup();
        if ids.binary_search(&primary).is_err() {
            return Err("created midi primary is missing");
        }
        self.replace(ids, Some(primary))
    }

    pub(super) fn retain_valid(
        &mut self,
        valid_ids: impl IntoIterator<Item = NoteId>,
    ) -> Result<bool, &'static str> {
        if self.ids.is_empty() {
            return Ok(false);
        }
        let mut valid = Vec::new();
        for id in valid_ids {
            if valid.len() >= hex_project::MAX_NOTES {
                return Err("project midi note count exceeds note limit");
            }
            valid
                .try_reserve(1)
                .map_err(|_| "could not reserve midi reconciliation")?;
            valid.push(id);
        }
        valid.sort_unstable();
        let mut next = Vec::new();
        next.try_reserve_exact(self.ids.len())
            .map_err(|_| "could not reserve midi reconciliation")?;
        next.extend(
            self.ids
                .iter()
                .copied()
                .filter(|id| valid.binary_search(id).is_ok()),
        );
        let primary = self
            .primary
            .filter(|id| next.binary_search(id).is_ok())
            .or_else(|| next.last().copied());
        self.replace(next, primary)
    }

    fn replace(&mut self, ids: Vec<NoteId>, primary: Option<NoteId>) -> Result<bool, &'static str> {
        if ids.len() > hex_project::MAX_NOTES {
            return Err("midi selection exceeds note limit");
        }
        if self.ids.as_ref() == ids.as_slice() && self.primary == primary {
            return Ok(false);
        }
        self.ids = Arc::from(ids);
        self.primary = primary;
        self.bump_epoch();
        Ok(true)
    }

    // Both callers supply an already validated, sorted immutable selection bank.
    fn replace_shared(&mut self, ids: Arc<[NoteId]>, primary: Option<NoteId>) -> bool {
        if (Arc::ptr_eq(&self.ids, &ids) || self.ids == ids) && self.primary == primary {
            return false;
        }
        self.ids = ids;
        self.primary = primary;
        self.bump_epoch();
        true
    }

    fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }
}

pub(super) fn marquee_hits(
    notes: &[NoteGeometry],
    first: [f32; 2],
    last: [f32; 2],
    width: f32,
    height: f32,
    pitch_rows: u8,
) -> Result<Vec<NoteId>, &'static str> {
    if !first.into_iter().chain(last).all(f32::is_finite)
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || pitch_rows == 0
    {
        return Ok(Vec::new());
    }
    let rect = [
        first[0].min(last[0]),
        first[1].min(last[1]),
        first[0].max(last[0]),
        first[1].max(last[1]),
    ];
    let mut hits = Vec::new();
    hits.try_reserve(notes.len().min(hex_project::MAX_NOTES))
        .map_err(|_| "could not reserve midi marquee hits")?;
    for note in notes {
        let (left, right) =
            midi_scene::pixel_bounds(*note, width).ok_or("invalid midi scene width")?;
        let top = note.y * height;
        let bottom = top + height / f32::from(pitch_rows);
        if right >= rect[0] && left <= rect[2] && bottom >= rect[1] && top <= rect[3] {
            hits.push(note.id);
        }
    }
    hits.sort_unstable();
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::{MidiSelection, marquee_hits};
    use crate::editor::midi_scene::NoteGeometry;
    use hex_project::{BeatTime, Command, Created, Editor, NoteId, Project};

    fn ids(count: usize) -> Vec<NoteId> {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .unwrap()
        else {
            panic!("track expected")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(16).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip expected")
        };
        (0..count)
            .map(|index| {
                let Created::Note(note) = editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_quarters(index as u64).unwrap(),
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap()
                else {
                    panic!("note expected")
                };
                note
            })
            .collect()
    }

    #[test]
    fn additive_toggle_select_all_and_clear_keep_primary_and_epoch_coherent() {
        let ids = ids(3);
        let mut selection = MidiSelection::default();
        assert!(selection.toggle(ids[0]).unwrap());
        assert!(selection.toggle(ids[2]).unwrap());
        assert_eq!(selection.ids(), &[ids[0], ids[2]]);
        assert_eq!(selection.primary(), Some(ids[2]));
        let epoch = selection.epoch();
        assert!(selection.toggle(ids[0]).unwrap());
        assert_eq!(selection.ids(), &[ids[2]]);
        assert_eq!(selection.primary(), Some(ids[2]));
        assert!(selection.epoch() > epoch);
        assert!(selection.select_all(ids.iter().copied()).unwrap());
        assert_eq!(selection.len(), 3);
        assert!(selection.clear());
        assert!(selection.is_empty());
    }

    #[test]
    fn additive_marquee_keeps_baseline_hits_and_unions_new_notes() {
        let ids = ids(4);
        let mut selection = MidiSelection::default();
        selection.set_single(ids[1]);
        assert!(selection.add_many(&[ids[1], ids[2], ids[3]]).unwrap());
        assert_eq!(selection.ids(), &[ids[1], ids[2], ids[3]]);
    }

    #[test]
    fn marquee_uses_intersection_with_visible_note_rectangles() {
        let ids = ids(2);
        let notes = [
            NoteGeometry {
                id: ids[0],
                active: true,
                start: 0,
                duration: 1,
                pitch: 71,
                channel: 0,
                velocity: 100,
                x: 0.2,
                y: 0.0,
                width: 0.1,
                leading_edge_visible: true,
                trailing_edge_visible: true,
            },
            NoteGeometry {
                id: ids[1],
                active: true,
                start: 1,
                duration: 1,
                pitch: 70,
                channel: 0,
                velocity: 100,
                x: 0.4,
                y: 1.0 / 24.0,
                width: 0.1,
                leading_edge_visible: true,
                trailing_edge_visible: true,
            },
        ];
        assert_eq!(
            marquee_hits(&notes, [29.0, 4.0], [21.0, 7.0], 100.0, 240.0, 24).unwrap(),
            [ids[0]]
        );
    }

    #[test]
    fn reconciliation_drops_cross_clip_or_deleted_ids_without_cloning_on_reads() {
        let ids = ids(3);
        let mut selection = MidiSelection::default();
        selection.select_all(ids.iter().copied()).unwrap();
        let shared = selection.shared_ids();
        assert!(selection.retain_valid([ids[0], ids[2]]).unwrap());
        assert_eq!(selection.ids(), &[ids[0], ids[2]]);
        assert_eq!(shared.as_ref(), ids.as_slice());
    }
}
