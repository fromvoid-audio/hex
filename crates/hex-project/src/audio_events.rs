//! Non-destructive intervals within one retained audio processing root.
//! IDs are local to this sequence; callers scope references to clip/revision.
use crate::{AudioClipEdit, Clip, ClipId, Invalid, Project};
use std::sync::Arc;

pub const MAX_AUDIO_EVENTS: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioEvent {
    pub id: u64,
    /// Half-open positions in the retained root's clock.
    pub start: u64,
    pub end: u64,
    /// Original root coordinate played at `start` (never a mutable PCM offset).
    pub source_start: u64,
}
impl AudioEvent {
    pub fn length(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioEvents {
    pub next_id: u64,
    pub events: Vec<AudioEvent>,
}

#[derive(Clone, Debug)]
pub enum AudioEventEdit {
    Split {
        at: u64,
    },
    SplitRange {
        start: u64,
        end: u64,
    },
    Move {
        event: u64,
        start: u64,
    },
    MoveRange {
        start: u64,
        end: u64,
        destination: u64,
    },
    Trim {
        event: u64,
        start: u64,
        end: u64,
    },
    Delete {
        start: u64,
        end: u64,
    },
    /// Clipboard coordinates are relative to its selected range; sources refer
    /// to this same retained root. Cross-root paste needs explicit source custody.
    Insert {
        at: u64,
        length: u64,
        events: Vec<AudioEvent>,
    },
    Paste {
        at: u64,
        length: u64,
        events: Vec<AudioEvent>,
    },
}
/// One canonical interior split, shared with the borrowed editable view.
#[derive(Clone, Copy)]
pub(crate) struct EventSplit {
    pub left: AudioEvent,
    pub right: AudioEvent,
    pub next_id: u64,
}
pub(crate) fn split_event(
    event: AudioEvent,
    at: u64,
    next_id: u64,
) -> Result<Option<EventSplit>, Invalid> {
    if event.start >= at || at >= event.end {
        return Ok(None);
    }
    let following = next_id
        .checked_add(1)
        .ok_or(Invalid("audio event identity exhausted"))?;
    Ok(Some(EventSplit {
        left: AudioEvent { end: at, ..event },
        right: AudioEvent {
            id: next_id,
            start: at,
            source_start: event.source_start + (at - event.start),
            ..event
        },
        next_id: following,
    }))
}
pub(crate) fn validate_event_geometry(
    next_id: u64,
    length: u64,
    events: impl IntoIterator<Item = Result<AudioEvent, Invalid>>,
    mut unique: impl FnMut(usize, u64) -> bool,
) -> Result<(), Invalid> {
    if next_id == 0 {
        return Err(Invalid("audio event identity is invalid"));
    }
    let mut previous_end = 0;
    for (index, event) in events.into_iter().enumerate() {
        let event = event?;
        if event.id == 0
            || event.id >= next_id
            || !unique(index, event.id)
            || event.start < previous_end
            || event.start >= event.end
            || event.end > length
            || event
                .source_start
                .checked_add(event.length())
                .is_none_or(|end| end > length)
        {
            return Err(Invalid(
                "audio events must have unique identities and bounded nonoverlapping intervals",
            ));
        }
        previous_end = event.end;
    }
    Ok(())
}
impl AudioEvents {
    pub fn identity(length: u64) -> Self {
        Self {
            next_id: 2,
            events: vec![AudioEvent {
                id: 1,
                start: 0,
                end: length,
                source_start: 0,
            }],
        }
    }
    pub fn validate(&self, length: u64) -> Result<(), Invalid> {
        if self.events.len() > MAX_AUDIO_EVENTS || self.next_id == 0 {
            return Err(Invalid("audio event limit or identity is invalid"));
        }
        self.validate_transformed(length, Ok)
    }
    /// Full-domain validation uses bounded stack scratch, including virtual
    /// affine events. Temporary edit fragments retain their separate validator.
    /// 1024 (u64, usize) entries occupy 16 KiB on supported 64-bit hosts.
    pub(crate) fn validate_transformed(
        &self,
        length: u64,
        transform: impl Fn(AudioEvent) -> Result<AudioEvent, Invalid>,
    ) -> Result<(), Invalid> {
        if self.events.len() > MAX_AUDIO_EVENTS || self.next_id == 0 {
            return Err(Invalid("audio event limit or identity is invalid"));
        }
        let mut ids = [(0_u64, 0_usize); MAX_AUDIO_EVENTS];
        for (index, event) in self.events.iter().enumerate() {
            ids[index] = (event.id, index);
        }
        let ids = &mut ids[..self.events.len()];
        ids.sort_unstable();
        let duplicate = ids
            .windows(2)
            .filter(|pair| pair[0].0 == pair[1].0)
            .map(|pair| pair[1].1)
            .min();
        self.validate_geometry_with(length, transform, |index, _| Some(index) != duplicate)
    }
    fn validate_geometry(&self, length: u64) -> Result<(), Invalid> {
        if self.next_id == 0 {
            return Err(Invalid("audio event identity is invalid"));
        }
        let mut ids = std::collections::BTreeSet::new();
        self.validate_geometry_with(length, Ok, |_, id| ids.insert(id))
    }
    fn validate_geometry_with(
        &self,
        length: u64,
        transform: impl Fn(AudioEvent) -> Result<AudioEvent, Invalid>,
        unique: impl FnMut(usize, u64) -> bool,
    ) -> Result<(), Invalid> {
        validate_event_geometry(
            self.next_id,
            length,
            self.events.iter().copied().map(transform),
            unique,
        )
    }

    fn id(&mut self) -> Result<u64, Invalid> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or(Invalid("audio event identity exhausted"))?;
        Ok(id)
    }
    fn carve(&mut self, start: u64, end: u64) -> Result<(), Invalid> {
        let mut result = Vec::new();
        for event in std::mem::take(&mut self.events) {
            if event.end <= start || event.start >= end {
                result.push(event);
                continue;
            }
            if event.start < start {
                result.push(AudioEvent {
                    end: start,
                    ..event
                });
            }
            if event.end > end {
                result.push(AudioEvent {
                    id: self.id()?,
                    start: end,
                    source_start: event.source_start + (end - event.start),
                    ..event
                });
            }
        }
        self.events = result;
        Ok(())
    }
    pub fn edited(&self, operation: AudioEventEdit, length: u64) -> Result<Self, Invalid> {
        // Cropped views may add two virtual boundary fragments. Only the final
        // canonical result consumes the event limit, so reducing edits stay usable.
        if self.events.len() > MAX_AUDIO_EVENTS + 2 {
            return Err(Invalid("audio event view limit reached"));
        }
        let next = self.edited_inner(operation, length)?;
        next.validate(length)?;
        Ok(next)
    }
    fn edited_inner(&self, operation: AudioEventEdit, length: u64) -> Result<Self, Invalid> {
        self.validate_geometry(length)?;
        let mut next = self.clone();
        match operation {
            AudioEventEdit::Split { at } => {
                if let Some(index) = next.events.iter().position(|e| e.start < at && at < e.end) {
                    let event = next.events[index];
                    if let Some(split) = split_event(event, at, next.next_id)? {
                        next.next_id = split.next_id;
                        next.events[index] = split.left;
                        next.events.insert(index + 1, split.right);
                    }
                }
            }
            AudioEventEdit::SplitRange { start, end } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event split range"));
                }
                next = next.edited_inner(AudioEventEdit::Split { at: start }, length)?;
                next = next.edited_inner(AudioEventEdit::Split { at: end }, length)?;
            }
            AudioEventEdit::Insert {
                at,
                length: span,
                events,
            } => {
                if at > length || span == 0 {
                    return Err(Invalid("invalid audio event insert range"));
                }
                next = next.edited_inner(AudioEventEdit::Split { at }, length)?;
                for event in &mut next.events {
                    if event.start >= at {
                        event.end = event
                            .end
                            .checked_add(span)
                            .filter(|end| *end <= length)
                            .ok_or(Invalid(
                                "audio event insert needs free time inside the clip",
                            ))?;
                        event.start = event
                            .start
                            .checked_add(span)
                            .ok_or(Invalid("audio event insert position overflow"))?;
                    }
                }
                next = next.edited_inner(
                    AudioEventEdit::Paste {
                        at,
                        length: span,
                        events,
                    },
                    length,
                )?;
            }
            AudioEventEdit::Move { event, start } => {
                let index = next
                    .events
                    .iter()
                    .position(|e| e.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let mut moved = next.events.remove(index);
                let end = start
                    .checked_add(moved.length())
                    .filter(|end| *end <= length)
                    .ok_or(Invalid("audio event move exceeds its container"))?;
                if start == moved.start {
                    return Ok(self.clone());
                }
                next.carve(start, end)?;
                moved.start = start;
                moved.end = end;
                next.events.push(moved);
            }
            AudioEventEdit::Trim { event, start, end } => {
                let index = next
                    .events
                    .iter()
                    .position(|e| e.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let original = next.events[index];
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event trim"));
                }
                let source_start = if start < original.start {
                    original.source_start.checked_sub(original.start - start)
                } else {
                    original.source_start.checked_add(start - original.start)
                }
                .ok_or(Invalid("audio event trim exceeds original source"))?;
                next.events.remove(index);
                next.carve(start, end)?;
                next.events.push(AudioEvent {
                    start,
                    end,
                    source_start,
                    ..original
                });
            }
            AudioEventEdit::MoveRange {
                start,
                end,
                destination,
            } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event time range"));
                }
                let span = end - start;
                if destination == start {
                    return Ok(self.clone());
                }
                destination
                    .checked_add(span)
                    .filter(|end| *end <= length)
                    .ok_or(Invalid("audio event range move exceeds its container"))?;
                let copied = next.copy_range(start, end);
                next = next.edited_inner(AudioEventEdit::Delete { start, end }, length)?;
                next = next.edited_inner(
                    AudioEventEdit::Paste {
                        at: destination,
                        length: span,
                        events: copied,
                    },
                    length,
                )?;
            }
            AudioEventEdit::Delete { start, end } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event time range"));
                }
                next.carve(start, end)?;
            }
            AudioEventEdit::Paste {
                at,
                length: span,
                events,
            } => {
                let end = at
                    .checked_add(span)
                    .filter(|end| *end <= length && span > 0)
                    .ok_or(Invalid("audio event paste exceeds its container"))?;
                if events.len() > MAX_AUDIO_EVENTS {
                    return Err(Invalid("audio event clipboard limit reached"));
                }
                // Validate untrusted clipboard geometry before carving anything.
                let mut prior = 0;
                for event in &events {
                    if event.start < prior
                        || event.start >= event.end
                        || event.end > span
                        || event
                            .source_start
                            .checked_add(event.length())
                            .is_none_or(|end| end > length)
                    {
                        return Err(Invalid("invalid audio event clipboard"));
                    }
                    prior = event.end;
                }
                next.carve(at, end)?;
                for event in events {
                    let id = next.id()?;
                    next.events.push(AudioEvent {
                        id,
                        start: at + event.start,
                        end: at + event.end,
                        ..event
                    });
                }
            }
        }
        next.events.sort_by_key(|e| e.start);
        next.validate_geometry(length)?;
        Ok(next)
    }
    pub fn copy_range(&self, start: u64, end: u64) -> Vec<AudioEvent> {
        self.events
            .iter()
            .filter_map(|event| {
                let a = start.max(event.start);
                let b = end.min(event.end);
                (a < b).then(|| AudioEvent {
                    start: a - start,
                    end: b - start,
                    source_start: event.source_start + (a - event.start),
                    ..*event
                })
            })
            .collect()
    }
}

impl Clip {
    pub fn audio_event_root(&self) -> AudioClipEdit {
        self.audio_edit()
            .cloned()
            .unwrap_or_else(|| AudioClipEdit::capture(self))
    }
    /// Split only the editable view boundaries, retaining hidden source intervals.
    /// The returned local IDs are materialized by the next actual inner edit.
    pub fn editable_audio_events(&self) -> Result<AudioEvents, Invalid> {
        let root = self.audio_event_root();
        if root.view_start == 0 && root.view_end == root.domain.clock.length() {
            return Ok(self.audio_events());
        }
        self.audio_events().edited_inner(
            AudioEventEdit::SplitRange {
                start: root.view_start,
                end: root.view_end,
            },
            root.domain.clock.length(),
        )
    }
    /// The implicit initial event keeps legacy clips allocation-free until edited.
    pub fn audio_events(&self) -> AudioEvents {
        self.audio_edit()
            .and_then(|edit| edit.domain.events.as_deref())
            .cloned()
            .unwrap_or_else(|| {
                AudioEvents::identity(
                    self.audio_edit()
                        .map_or(self.duration(), |e| e.domain.clock.length()),
                )
            })
    }
}

pub(crate) fn apply(
    project: &mut Project,
    clip: ClipId,
    operation: AudioEventEdit,
) -> Result<(), Invalid> {
    let original = project.clip(clip).ok_or(Invalid("clip no longer exists"))?;
    let mut edit = original
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(original));
    let events = original.editable_audio_events()?;
    let next = events.edited(operation, edit.domain.clock.length())?;
    if next == events {
        return Ok(());
    }
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(next));
    let asset = project
        .asset(original.asset())
        .ok_or(Invalid("audio event source missing"))?;
    edit.validate(original, project.tempo_map(), asset.playback_frames())?;
    project.clip_mut(clip)?.edit = Some(Arc::new(edit));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_move_overwrite_trim_and_range_clipboard_preserve_source_coordinates() {
        let identity = AudioEvents::identity(100);
        let split = identity
            .edited(AudioEventEdit::Split { at: 30 }, 100)
            .unwrap();
        assert_eq!(
            split
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 30, 0), (30, 100, 30)]
        );
        let moved = split
            .edited(
                AudioEventEdit::Move {
                    event: 1,
                    start: 50,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            moved
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(30, 50, 30), (50, 80, 0), (80, 100, 80)]
        );
        let trim = moved
            .edited(
                AudioEventEdit::Trim {
                    event: 1,
                    start: 55,
                    end: 75,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            trim.events.iter().find(|e| e.id == 1).unwrap().source_start,
            5
        );
        let copied = trim.copy_range(40, 85);
        let pasted = trim
            .edited(
                AudioEventEdit::Paste {
                    at: 0,
                    length: 45,
                    events: copied,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            pasted
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![
                (0, 10, 40),
                (15, 35, 5),
                (40, 45, 80),
                (45, 50, 45),
                (55, 75, 5),
                (80, 100, 80)
            ]
        );
        let cut = pasted
            .edited(AudioEventEdit::Delete { start: 10, end: 80 }, 100)
            .unwrap();
        assert_eq!(
            cut.events
                .iter()
                .map(|e| (e.start, e.end))
                .collect::<Vec<_>>(),
            vec![(0, 10), (80, 100)]
        );
        assert!(
            cut.edited(
                AudioEventEdit::Move {
                    event: 1,
                    start: u64::MAX
                },
                100
            )
            .is_err()
        );
        assert_eq!(identity.events[0].end, 100);
    }
    #[test]
    fn canonical_events_undo_and_outer_splits_keep_the_full_retained_sequence() {
        use crate::{Command, Created, Editor};
        let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "test".into(),
                path: "test.wav".into(),
                sample_rate: 48000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 30 },
            })
            .unwrap();
        let events = editor.project().clip(clip).unwrap().audio_events();
        assert_eq!(events.events.len(), 2);
        assert!(editor.undo());
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
        assert!(editor.redo());
        let Created::Clip(right) = editor
            .execute(Command::SplitClip { clip, frame: 50 })
            .unwrap()
        else {
            panic!("split")
        };
        assert_eq!(editor.project().clip(clip).unwrap().audio_events(), events);
        assert_eq!(editor.project().clip(right).unwrap().audio_events(), events);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    #[test]
    fn range_move_and_insert_preserve_holes_and_source_coordinates_with_finite_capacity() {
        let initial = AudioEvents::identity(100);
        let split = initial
            .edited(AudioEventEdit::SplitRange { start: 20, end: 40 }, 100)
            .unwrap();
        assert_eq!(
            split
                .events
                .iter()
                .map(|e| (e.start, e.end))
                .collect::<Vec<_>>(),
            vec![(0, 20), (20, 40), (40, 100)]
        );
        let cut = split
            .edited(AudioEventEdit::Delete { start: 40, end: 60 }, 100)
            .unwrap();
        let moved = cut
            .edited(
                AudioEventEdit::MoveRange {
                    start: 20,
                    end: 60,
                    destination: 60,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            moved
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 20, 0), (60, 80, 20)]
        );
        let clipboard = moved.copy_range(0, 20);
        let inserted = moved
            .edited(
                AudioEventEdit::Insert {
                    at: 0,
                    length: 20,
                    events: clipboard,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            inserted
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 20, 0), (20, 40, 0), (80, 100, 20)]
        );
        assert!(
            inserted
                .edited(
                    AudioEventEdit::Insert {
                        at: 0,
                        length: 1,
                        events: vec![]
                    },
                    100
                )
                .is_err()
        );
        let moved_id = moved.events.iter().find(|e| e.start == 60).unwrap().id;
        let extended = moved
            .edited(
                AudioEventEdit::Trim {
                    event: moved_id,
                    start: 55,
                    end: 85,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            extended
                .events
                .iter()
                .find(|e| e.id == moved_id)
                .unwrap()
                .source_start,
            15
        );
    }
    #[test]
    fn splitting_at_capacity_refuses_without_mutating_the_sequence() {
        let sequence = AudioEvents {
            next_id: MAX_AUDIO_EVENTS as u64 + 1,
            events: (0..MAX_AUDIO_EVENTS as u64)
                .map(|i| AudioEvent {
                    id: i + 1,
                    start: i * 4,
                    end: i * 4 + 2,
                    source_start: i * 4,
                })
                .collect(),
        };
        sequence.validate(MAX_AUDIO_EVENTS as u64 * 4).unwrap();
        assert!(
            sequence
                .edited(AudioEventEdit::Split { at: 1 }, MAX_AUDIO_EVENTS as u64 * 4)
                .is_err()
        );
        assert_eq!(sequence.events.len(), MAX_AUDIO_EVENTS);
    }
}

#[cfg(test)]
mod cropped_capacity_regression {
    use super::*;
    use crate::{Command, Created, Editor};
    #[test]
    fn delete_from_cropped_full_sequence_checks_final_capacity_and_undo() {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "capacity".into(),
                path: "capacity.wav".into(),
                sample_rate: 48000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        let mut project = editor.project().clone();
        let mut root = project.clip(clip).unwrap().audio_event_root();
        Arc::make_mut(&mut root.domain).events = Some(Arc::new(AudioEvents {
            next_id: 1025,
            events: (0..1024)
                .map(|i| AudioEvent {
                    id: i + 1,
                    start: i * 4,
                    end: i * 4 + 4,
                    source_start: i * 4,
                })
                .collect(),
        }));
        project.clip_mut(clip).unwrap().edit = Some(Arc::new(root));
        let mut editor = Editor::new(project, 8).unwrap();
        let Created::Clip(right) = editor
            .execute(Command::SplitClip { clip, frame: 2 })
            .unwrap()
        else {
            panic!("split")
        };
        let before = editor.project().clone();
        assert!(
            editor
                .project()
                .clip(right)
                .unwrap()
                .editable_audio_events()
                .is_ok(),
            "virtual crop boundaries must remain editable at capacity"
        );
        assert!(
            editor
                .execute(Command::EditAudioEvents {
                    clip: right,
                    operation: AudioEventEdit::Split { at: 3 },
                })
                .is_err(),
            "an increasing edit must still enforce the final limit"
        );
        assert_eq!(*editor.project(), before);
        editor
            .execute(Command::EditAudioEvents {
                clip: right,
                operation: AudioEventEdit::Delete {
                    start: 2,
                    end: 4096,
                },
            })
            .unwrap();
        let sequence = editor.project().clip(right).unwrap().audio_events();
        assert_eq!(sequence.events.len(), 1);
        assert_eq!((sequence.events[0].start, sequence.events[0].end), (0, 2));
        sequence.validate(4096).unwrap();
        assert!(editor.undo());
        assert_eq!(*editor.project(), before);
    }
}
