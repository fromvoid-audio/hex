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
        let mut ids = std::collections::BTreeSet::new();
        let mut previous_end = 0;
        for event in &self.events {
            if event.id == 0
                || event.id >= self.next_id
                || !ids.insert(event.id)
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
        self.validate(length)?;
        let mut next = self.clone();
        match operation {
            AudioEventEdit::Split { at } => {
                if let Some(index) = next.events.iter().position(|e| e.start < at && at < e.end) {
                    let event = next.events[index];
                    let right = AudioEvent {
                        id: next.id()?,
                        start: at,
                        source_start: event.source_start + (at - event.start),
                        ..event
                    };
                    next.events[index].end = at;
                    next.events.insert(index + 1, right);
                }
            }
            AudioEventEdit::SplitRange { start, end } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event split range"));
                }
                next = next.edited(AudioEventEdit::Split { at: start }, length)?;
                next = next.edited(AudioEventEdit::Split { at: end }, length)?;
            }
            AudioEventEdit::Insert {
                at,
                length: span,
                events,
            } => {
                if at > length || span == 0 {
                    return Err(Invalid("invalid audio event insert range"));
                }
                next = next.edited(AudioEventEdit::Split { at }, length)?;
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
                next = next.edited(
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
                next = next.edited(AudioEventEdit::Delete { start, end }, length)?;
                next = next.edited(
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
        next.validate(length)?;
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
    let events = original.audio_events();
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
