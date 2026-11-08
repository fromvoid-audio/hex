//! Persistent source identity is independent of every clip placement. The source
//! registry owns immutable content; a separate clip-ID index owns membership.
//! Explicit parent application copies the picked member's current local settings
//! to the group. There is no distinguished original clip or hidden reset default.
use crate::{
    AliasSourceId, AssetId, AudioClipContent, BeatTime, ClipId, Invalid, MidiContent, MidiNote,
    MidiNoteSnapshot, NoteId, Project,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Content {
    Audio {
        asset: AssetId,
        content: Arc<AudioClipContent>,
    },
    Midi(Arc<MidiContent>),
    ConsoleAudio(AssetId),
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClipAliases {
    pub(crate) sources: BTreeMap<AliasSourceId, Content>,
    pub(crate) members: BTreeMap<ClipId, AliasSourceId>,
}
/// Complete source payloads are stored once; member MIDI snapshots contain no
/// inline notes. A reference is explicit even when its source has no notes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipAliasesSnapshot {
    pub sources: Vec<AliasSourceSnapshot>,
    pub members: Vec<AliasMemberSnapshot>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasSourceSnapshot {
    pub id: u64,
    pub content: AliasContentSnapshot,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AliasContentSnapshot {
    Audio {
        asset: u64,
        content: Arc<AudioClipContent>,
    },
    Midi {
        source_length_raw: u64,
        notes: Vec<MidiNoteSnapshot>,
    },
    ConsoleAudio {
        asset: u64,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AliasMemberSnapshot {
    pub clip: u64,
    pub source: u64,
}
impl Project {
    /// Whether the project requires an alias-aware saved contract. Includes
    /// both inventories so an inconsistent private candidate cannot downgrade.
    pub fn has_clip_aliases(&self) -> bool {
        !self.aliases.sources.is_empty() || !self.aliases.members.is_empty()
    }
    /// Explicit persistent source membership, never inferred from pointer sharing.
    pub fn clip_alias(&self, clip: ClipId) -> Option<AliasSourceId> {
        self.aliases.members.get(&clip).copied()
    }
}
impl ClipAliases {
    pub(crate) fn snapshot(&self) -> ClipAliasesSnapshot {
        ClipAliasesSnapshot {
            sources: self
                .sources
                .iter()
                .map(|(id, source)| AliasSourceSnapshot {
                    id: id.value(),
                    content: match source {
                        Content::Audio { asset, content } => AliasContentSnapshot::Audio {
                            asset: asset.value(),
                            content: Arc::clone(content),
                        },
                        Content::Midi(content) => AliasContentSnapshot::Midi {
                            source_length_raw: content.source_length.raw(),
                            notes: content
                                .notes
                                .iter()
                                .map(|note| MidiNoteSnapshot {
                                    id: note.id.value(),
                                    active: note.active,
                                    start_raw: note.start.raw(),
                                    duration_raw: note.duration.raw(),
                                    pitch: note.pitch,
                                    channel: note.channel,
                                    velocity: note.velocity,
                                })
                                .collect(),
                        },
                        Content::ConsoleAudio(asset) => AliasContentSnapshot::ConsoleAudio {
                            asset: asset.value(),
                        },
                    },
                })
                .collect(),
            members: self
                .members
                .iter()
                .map(|(clip, source)| AliasMemberSnapshot {
                    clip: clip.value(),
                    source: source.value(),
                })
                .collect(),
        }
    }
    pub(crate) fn from_snapshot(
        snapshot: ClipAliasesSnapshot,
        ids: &mut BTreeSet<u64>,
        max_id: &mut u64,
    ) -> Result<(Self, usize), Invalid> {
        if snapshot.sources.len() > crate::MAX_CLIPS || snapshot.members.len() > crate::MAX_CLIPS {
            return Err(Invalid("clip alias inventory exceeds project limits"));
        }
        let mut result = Self::default();
        let mut note_count = 0usize;
        for source in snapshot.sources {
            crate::snapshot::insert_id(source.id, ids, max_id)?;
            let content = match source.content {
                AliasContentSnapshot::Audio { asset, content } => {
                    if matches!(content.clock, crate::AudioDomainClock::Frames(length)
                        if length > crate::MAX_AUDIO_DOMAIN_FRAMES)
                    {
                        return Err(Invalid(
                            "linked audio exceeds the supported frame event extent",
                        ));
                    }
                    Content::Audio {
                        asset: AssetId(asset),
                        content,
                    }
                }
                AliasContentSnapshot::ConsoleAudio { asset } => {
                    Content::ConsoleAudio(AssetId(asset))
                }
                AliasContentSnapshot::Midi {
                    source_length_raw,
                    notes,
                } => {
                    let length = BeatTime::from_raw(source_length_raw);
                    if length == BeatTime::ZERO {
                        return Err(Invalid("alias midi source must have positive length"));
                    }
                    note_count = note_count
                        .checked_add(notes.len())
                        .filter(|count| *count <= crate::MAX_NOTES)
                        .ok_or(Invalid("alias note limit reached"))?;
                    let mut compiled = Vec::new();
                    compiled
                        .try_reserve_exact(notes.len())
                        .map_err(|_| Invalid("could not reserve alias notes"))?;
                    for note in notes {
                        crate::snapshot::insert_id(note.id, ids, max_id)?;
                        let start = BeatTime::from_raw(note.start_raw);
                        let duration = BeatTime::from_raw(note.duration_raw);
                        crate::midi::validate_note(
                            start,
                            duration,
                            length,
                            note.pitch,
                            note.channel,
                            note.velocity,
                        )?;
                        compiled.push(MidiNote {
                            id: NoteId(note.id),
                            active: note.active,
                            start,
                            duration,
                            pitch: note.pitch,
                            channel: note.channel,
                            velocity: note.velocity,
                        });
                    }
                    crate::midi_note_overwrite::validate_occupancy(&compiled)?;
                    Content::Midi(Arc::new(MidiContent {
                        source_length: length,
                        notes: compiled,
                    }))
                }
            };
            if result
                .sources
                .insert(AliasSourceId(source.id), content)
                .is_some()
            {
                return Err(Invalid("duplicate alias source"));
            }
        }
        for member in snapshot.members {
            if member.clip == 0
                || !result.sources.contains_key(&AliasSourceId(member.source))
                || result
                    .members
                    .insert(ClipId(member.clip), AliasSourceId(member.source))
                    .is_some()
            {
                return Err(Invalid("invalid or duplicate alias member reference"));
            }
        }
        if result
            .sources
            .keys()
            .any(|id| !result.members.values().any(|member| member == id))
        {
            return Err(Invalid("orphan alias source"));
        }
        Ok((result, note_count))
    }
    pub(crate) fn midi_content(&self, clip: ClipId) -> Result<Option<Arc<MidiContent>>, Invalid> {
        let Some(id) = self.members.get(&clip) else {
            return Ok(None);
        };
        match self.sources.get(id) {
            Some(Content::Midi(content)) => Ok(Some(Arc::clone(content))),
            _ => Err(Invalid("alias source kind differs from midi member")),
        }
    }
    pub(crate) fn validate_and_bind(&self, project: &mut Project) -> Result<(), Invalid> {
        for (clip, source) in &self.members {
            match self.sources.get(source).ok_or(Invalid("alias source missing"))? {
                Content::Midi(content) => {
                    let current = project.midi_source(*clip).ok_or(Invalid("alias midi member missing"))?;
                    if current.content.as_ref() != content.as_ref() { return Err(Invalid("alias midi member contradicts source")); }
                    if !Arc::ptr_eq(&current.content, content) { project.midi_source_mut(*clip)?.content = Arc::clone(content); }
                }
                Content::Audio { asset, content } => {
                    let target = project.clip(*clip).ok_or(Invalid("alias audio member missing"))?;
                    if target.asset != *asset { return Err(Invalid("alias audio media differs from source")); }
                    let edit = target.audio_edit().ok_or(Invalid("alias audio member has no processing root"))?;
                    if edit.domain.content.as_ref() != content.as_ref() { return Err(Invalid("alias audio member contradicts source")); }
                    if !Arc::ptr_eq(&edit.domain.content, content) {
                        let edit = project.clip_mut(*clip)?.audio_edit_mut().ok_or(Invalid("alias audio member has no processing root"))?;
                        Arc::make_mut(&mut edit.domain).content = Arc::clone(content);
                    }
                }
                Content::ConsoleAudio(asset) => {
                    if project.session.cells.iter().all(|cell| !matches!(&cell.content, crate::SessionCellContent::Audio(audio) if audio.id == *clip && audio.asset == *asset)) {
                        return Err(Invalid("alias console audio member differs from source"));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "clip_alias_tests.rs"]
mod tests;
