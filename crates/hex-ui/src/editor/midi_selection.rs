//! Bounded piano-roll note selection and marquee geometry.
use super::midi_scene::{self, NoteGeometry};
use hex_project::NoteId;
use std::sync::Arc;

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
