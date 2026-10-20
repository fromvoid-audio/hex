#[cfg(test)]
use hex_project::Project;
use hex_project::{ClipId, TrackId};
use hex_session::snapshot_exports::ui_consumers::SnapshotSessionCellContentRead;
use hex_session::snapshot_exports::{SnapshotProjectRead, SnapshotTransportError};

/// Finite selection facts; production reads never expose canonical owners.
pub(super) trait SelectionProject {
    fn track_ids(&self) -> impl ExactSizeIterator<Item = TrackId>;
    fn first_clip(&self, track: TrackId) -> Option<ClipId>;
    fn owns_clip(&self, track: TrackId, clip: ClipId) -> bool;
}
impl SelectionProject for SnapshotProjectRead<'_> {
    fn track_ids(&self) -> impl ExactSizeIterator<Item = TrackId> {
        self.tracks().map(|track| track.id())
    }
    fn first_clip(&self, track: TrackId) -> Option<ClipId> {
        let track = self.track(track)?;
        track
            .clips()
            .next()
            .map(|clip| clip.id())
            .or_else(|| track.midi_clips().next().map(|clip| clip.id()))
    }
    fn owns_clip(&self, track: TrackId, clip: ClipId) -> bool {
        self.track(track).is_some_and(|track| track.clips().any(|item| item.id() == clip) || track.midi_clips().any(|item| item.id() == clip))
            || self.session_cells().any(|cell| cell.track() == track && matches!(cell.content(), SnapshotSessionCellContentRead::Midi(item) if item.id() == clip))
    }
}
#[cfg(test)]
impl SelectionProject for Project {
    fn track_ids(&self) -> impl ExactSizeIterator<Item = TrackId> {
        self.tracks().iter().map(|track| track.id())
    }
    fn first_clip(&self, track: TrackId) -> Option<ClipId> {
        let track = self.track(track)?;
        track
            .clips()
            .first()
            .map(|clip| clip.id())
            .or_else(|| track.midi_clips().first().map(|clip| clip.id()))
    }
    fn owns_clip(&self, track: TrackId, clip: ClipId) -> bool {
        self.track(track).is_some_and(|track| track.clips().iter().any(|item| item.id() == clip) || track.midi_clips().iter().any(|item| item.id() == clip))
            || self.session_cells().iter().any(|cell| cell.track() == track && matches!(cell.content(), hex_project::SessionCellContent::Midi(item) if item.id() == clip))
    }
}

/// One view selection over canonical stable track IDs, independent of time ranges.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub(super) struct TrackSelection {
    ids: std::collections::BTreeSet<TrackId>,
    anchor: Option<TrackId>,
}

impl TrackSelection {
    pub(super) fn contains(&self, track: TrackId) -> bool {
        self.ids.contains(&track)
    }

    pub(super) fn ordered(&self, project: &(impl SelectionProject + ?Sized)) -> Vec<TrackId> {
        project
            .track_ids()
            .filter(|id| self.ids.contains(id))
            .collect()
    }

    pub(super) fn reconcile(&mut self, project: &(impl SelectionProject + ?Sized)) {
        self.ids
            .retain(|id| project.track_ids().any(|track| track == *id));
        if self
            .anchor
            .is_some_and(|id| !project.track_ids().any(|track| track == id))
        {
            self.anchor = self.ordered(project).first().copied();
        }
    }

    pub(super) fn select(
        &mut self,
        project: &(impl SelectionProject + ?Sized),
        track: TrackId,
        range: bool,
        toggle: bool,
    ) -> bool {
        let Some(index) = project.track_ids().position(|item| item == track) else {
            return false;
        };
        self.reconcile(project);
        if range {
            let anchor = self
                .anchor
                .and_then(|id| project.track_ids().position(|item| item == id))
                .unwrap_or(index);
            if !toggle {
                self.ids.clear();
            }
            for id in project
                .track_ids()
                .skip(anchor.min(index))
                .take(anchor.abs_diff(index) + 1)
            {
                self.ids.insert(id);
            }
            if self.anchor.is_none() {
                self.anchor = Some(track);
            }
        } else {
            if toggle {
                if !self.ids.remove(&track) {
                    self.ids.insert(track);
                }
            } else {
                self.ids.clear();
                self.ids.insert(track);
            }
            self.anchor = Some(track);
        }
        true
    }
}

impl super::EditorView {
    pub(super) fn update_track_selection_status(&mut self) {
        let Ok(ids) = self.selected_track_ids_checked() else {
            return;
        };
        self.status = match ids.len() {
            0 => "no tracks selected".into(),
            1 => "track selected".into(),
            count => format!("{count} tracks selected"),
        };
    }

    pub(super) fn select_track_with_modifiers(
        &mut self,
        track: TrackId,
        modifiers: gpui::Modifiers,
        window: &mut gpui::Window,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Ok(Some((selection, first_gain))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let first_gain = project
                .track(track)?
                .gain_points()
                .first()
                .map(|point| point.beat);
            let mut selection = self.track_selection.clone();
            selection
                .select(
                    &snapshot.model(),
                    track,
                    modifiers.shift,
                    modifiers.platform,
                )
                .then_some((selection, first_gain))
        }) else {
            return;
        };
        self.focus_selected_track(track, first_gain, window);
        self.track_selection = selection;
        self.update_track_selection_status();
    }

    /// Every consumer receives explicit members in canonical order; time range
    /// and focused-clip state never promote unrelated tracks into this selection.
    pub(super) fn selected_track_ids(&self) -> Vec<TrackId> {
        self.selected_track_ids_checked().unwrap_or_default()
    }
    pub(super) fn selected_track_ids_checked(
        &self,
    ) -> Result<Vec<TrackId>, SnapshotTransportError> {
        match self.state.as_ref() {
            Some(state) => {
                state.read_with(|snapshot| self.track_selection.ordered(&snapshot.model()))
            }
            None => Ok(Vec::new()),
        }
    }
}

/// Keep a valid selected clip attached to its canonical owner after edits,
/// undo, redo or worker-side project changes.
pub(super) fn reconcile(
    project: &(impl SelectionProject + ?Sized),
    current_track: Option<TrackId>,
    current_clip: Option<ClipId>,
) -> (Option<TrackId>, Option<ClipId>) {
    if let Some(clip_id) = current_clip
        && let Some(track) = project
            .track_ids()
            .find(|track| project.owns_clip(*track, clip_id))
    {
        return (Some(track), Some(clip_id));
    }
    let track = current_track
        .filter(|id| project.track_ids().any(|track| track == *id))
        .or_else(|| project.track_ids().next());
    (track, track.and_then(|id| project.first_clip(id)))
}

/// Keyboard navigation uses canonical order and stable IDs, without wrapping.
pub(super) fn neighbor(
    project: &(impl SelectionProject + ?Sized),
    current: Option<TrackId>,
    forward: bool,
) -> Option<(usize, TrackId, Option<ClipId>)> {
    let last = project.track_ids().len().checked_sub(1)?;
    let index = match current.and_then(|id| project.track_ids().position(|track| track == id)) {
        Some(index) if forward => index.saturating_add(1).min(last),
        Some(index) => index.saturating_sub(1),
        None if forward => 0,
        None => last,
    };
    let track = project.track_ids().nth(index)?;
    Some((index, track, project.first_clip(track)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn selection_handles_empty_stale_ids_boundaries_and_track_removal() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        assert!(neighbor(editor.project(), None, true).is_none());
        let mut ids = Vec::new();
        for name in ["first", "second", "third"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track id");
            };
            ids.push(id);
        }
        assert_eq!(neighbor(editor.project(), None, true).unwrap().1, ids[0]);
        assert_eq!(neighbor(editor.project(), None, false).unwrap().1, ids[2]);
        assert_eq!(
            neighbor(editor.project(), Some(ids[0]), false).unwrap().1,
            ids[0]
        );
        assert_eq!(
            neighbor(editor.project(), Some(ids[2]), true).unwrap().1,
            ids[2]
        );
        assert_eq!(
            neighbor(editor.project(), Some(ids[0]), true).unwrap().1,
            ids[1]
        );
        editor.execute(Command::RemoveTrack(ids[1])).unwrap();
        assert_eq!(
            neighbor(editor.project(), Some(ids[1]), true).unwrap().1,
            ids[0]
        );
        assert_eq!(
            neighbor(editor.project(), Some(ids[0]), true).unwrap().1,
            ids[2]
        );
    }

    #[test]
    fn reconciliation_follows_a_moved_clip_through_undo_and_redo() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("source track id");
        };
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("target track id");
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "Media/Original.WAV".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("asset id");
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceClip {
                track: source,
                asset,
                start: 10,
                duration: 40,
                source_offset: 3,
                source_length: 40,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip id");
        };

        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(source), Some(clip))
        );
        editor
            .execute(Command::MoveClip {
                clip,
                track: target,
                start: 70,
            })
            .unwrap();
        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(target), Some(clip))
        );
        assert!(editor.undo());
        assert_eq!(
            reconcile(editor.project(), Some(target), Some(clip)),
            (Some(source), Some(clip))
        );
        assert!(editor.redo());
        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(target), Some(clip))
        );
    }

    #[test]
    fn midi_clip_selection_follows_move_undo_and_redo_and_note_removal() {
        use hex_project::BeatTime;
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(source) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument a".into(),
            })
            .unwrap()
        else {
            panic!("source track");
        };
        let Created::Track(target) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument b".into(),
            })
            .unwrap()
        else {
            panic!("target track");
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip");
        };
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_ratio(1, 4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("midi note");
        };
        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(source), Some(clip))
        );
        editor
            .execute(Command::MoveMidiClip {
                clip,
                track: target,
                start: BeatTime::ONE,
            })
            .unwrap();
        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(target), Some(clip))
        );
        assert!(editor.undo());
        assert_eq!(
            reconcile(editor.project(), Some(target), Some(clip)),
            (Some(source), Some(clip))
        );
        assert!(editor.redo());
        assert_eq!(
            reconcile(editor.project(), Some(source), Some(clip)),
            (Some(target), Some(clip))
        );
        editor.execute(Command::RemoveMidiNote(note)).unwrap();
        assert!(editor.project().midi_note(note).is_none());
    }
}
