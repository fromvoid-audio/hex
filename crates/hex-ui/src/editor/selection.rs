use hex_project::{ClipId, Project, TrackId};

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

    pub(super) fn ordered(&self, project: &Project) -> Vec<TrackId> {
        project
            .tracks()
            .iter()
            .map(|track| track.id())
            .filter(|id| self.ids.contains(id))
            .collect()
    }

    pub(super) fn reconcile(&mut self, project: &Project) {
        self.ids.retain(|id| project.track(*id).is_some());
        if self.anchor.is_some_and(|id| project.track(id).is_none()) {
            self.anchor = self.ordered(project).first().copied();
        }
    }

    pub(super) fn select(
        &mut self,
        project: &Project,
        track: TrackId,
        range: bool,
        toggle: bool,
    ) -> bool {
        let Some(index) = project.tracks().iter().position(|item| item.id() == track) else {
            return false;
        };
        self.reconcile(project);
        if range {
            let anchor = self
                .anchor
                .and_then(|id| project.tracks().iter().position(|item| item.id() == id))
                .unwrap_or(index);
            if !toggle {
                self.ids.clear();
            }
            for item in &project.tracks()[anchor.min(index)..=anchor.max(index)] {
                self.ids.insert(item.id());
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
    pub(super) fn select_track_with_modifiers(
        &mut self,
        track: TrackId,
        modifiers: gpui::Modifiers,
        window: &mut gpui::Window,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self
            .track_selection
            .select(&state.project, track, modifiers.shift, modifiers.platform)
        {
            self.focus_selected_track(track, window);
            let count = self.selected_track_ids().len();
            self.status = match count {
                0 => "no tracks selected".into(),
                1 => "track selected".into(),
                count => format!("{count} tracks selected"),
            };
        }
    }

    /// Every consumer receives explicit members in canonical order; time range
    /// and focused-clip state never promote unrelated tracks into this selection.
    pub(super) fn selected_track_ids(&self) -> Vec<TrackId> {
        self.state.as_ref().map_or_else(Vec::new, |state| {
            self.track_selection.ordered(&state.project)
        })
    }
}

/// Keep a valid selected clip attached to its canonical owner after edits,
/// undo, redo or worker-side project changes.
pub(super) fn reconcile(
    project: &Project,
    current_track: Option<TrackId>,
    current_clip: Option<ClipId>,
) -> (Option<TrackId>, Option<ClipId>) {
    if let Some(clip_id) = current_clip
        && let Some(track) = project.tracks().iter().find(|track| {
            track.clips().iter().any(|clip| clip.id() == clip_id)
                || track.midi_clips().iter().any(|clip| clip.id() == clip_id)
                || project.session_cells().iter().any(|cell| {
                    cell.track() == track.id()
                        && matches!(cell.content(), hex_project::SessionCellContent::Midi(clip) if clip.id() == clip_id)
                })
        })
    {
        return (Some(track.id()), Some(clip_id));
    }
    let track = current_track
        .filter(|id| project.track(*id).is_some())
        .or_else(|| project.tracks().first().map(|track| track.id()));
    let clip = track.and_then(|id| {
        let track = project.track(id)?;
        track
            .clips()
            .first()
            .map(|clip| clip.id())
            .or_else(|| track.midi_clips().first().map(|clip| clip.id()))
    });
    (track, clip)
}

/// Keyboard navigation uses canonical order and stable IDs, without wrapping.
pub(super) fn neighbor(
    project: &Project,
    current: Option<TrackId>,
    forward: bool,
) -> Option<(usize, TrackId, Option<ClipId>)> {
    let tracks = project.tracks();
    let last = tracks.len().checked_sub(1)?;
    let index = match current.and_then(|id| tracks.iter().position(|track| track.id() == id)) {
        Some(index) if forward => index.saturating_add(1).min(last),
        Some(index) => index.saturating_sub(1),
        None if forward => 0,
        None => last,
    };
    let track = tracks.get(index)?;
    Some((
        index,
        track.id(),
        track
            .clips()
            .first()
            .map(|clip| clip.id())
            .or_else(|| track.midi_clips().first().map(|clip| clip.id())),
    ))
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
