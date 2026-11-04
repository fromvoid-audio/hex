//! Command-scoped shared writes reconcile only after complete selection rows
//! return. Ordered batch commands reconcile separately before the next command.
use crate::clip_alias::Content;
use crate::{
    AliasSourceId, BeatTime, Clip, ClipCommandScope, ClipEdit, ClipId, Command, Invalid, Project,
};
use std::{collections::BTreeSet, sync::Arc};

struct Write {
    source: AliasSourceId,
    clip: ClipId,
    command: Command,
    before_audio: Option<Clip>,
    prefix: BeatTime,
}
pub(crate) struct Plan {
    writes: Vec<Write>,
    explicit: bool,
}
fn shared(edit: &ClipEdit) -> bool {
    matches!(
        edit,
        ClipEdit::Pitch { .. }
            | ClipEdit::PitchBy { .. }
            | ClipEdit::Musical { .. }
            | ClipEdit::SampleBpm { .. }
            | ClipEdit::WarpEnabled { .. }
            | ClipEdit::WarpMode { .. }
            | ClipEdit::WarpAdjust { .. }
    )
}
fn content(project: &Project, clip: ClipId) -> Option<Content> {
    if let Some(source) = project.midi_source(clip) {
        Some(Content::Midi(Arc::clone(&source.content)))
    } else if let Some(clip) = project.clip(clip) {
        clip.edit.as_ref().map(|edit| Content::Audio {
            asset: clip.asset,
            content: Arc::clone(&edit.domain.content),
        })
    } else {
        project
            .session
            .cells
            .iter()
            .find_map(|cell| match &cell.content {
                crate::SessionCellContent::Audio(audio) if audio.id == clip => {
                    Some(Content::ConsoleAudio(audio.asset))
                }
                _ => None,
            })
    }
}
impl Plan {
    pub(crate) fn capture(project: &Project, command: &mut Command) -> Result<Self, Invalid> {
        let explicit = matches!(
            command,
            Command::DuplicateAliasClips(_)
                | Command::DuplicateAliasClipsAt { .. }
                | Command::DuplicateAliasClipToNewTrack { .. }
                | Command::MakeClipsUnique(_)
                | Command::ApplyClipSettingsToAliases(_)
                | Command::DuplicateSessionCell { alias: true, .. }
        );
        if !project.has_clip_aliases() {
            return Ok(Self {
                writes: Vec::new(),
                explicit,
            });
        }
        let inspect = match command.clip_scope() {
            ClipCommandScope::Unrelated => {
                matches!(
                    command,
                    Command::RemoveTrack(_)
                        | Command::RemoveTracks(_)
                        | Command::ImportPreparedAudioClipWithOverwrite { .. }
                        | Command::ImportPreparedSessionAudioClip { .. }
                        | Command::CommitArrangementBounce { .. }
                        | Command::RemoveScene(_)
                        | Command::ClearSessionCell { .. }
                        | Command::SetSessionStopCell { .. }
                        | Command::MoveSessionCell { .. }
                        | Command::SetProjectTempo { .. }
                        | Command::SetTempoPoint { .. }
                ) || matches!(
                    command,
                    Command::ImportExistingAudio { placement, .. } if placement.may_replace_clips()
                )
            }
            ClipCommandScope::Property { edit, .. } => {
                shared(&edit) || matches!(edit, ClipEdit::Delete)
            }
            _ => true,
        } || explicit;
        if !inspect {
            return Ok(Self {
                writes: Vec::new(),
                explicit,
            });
        }
        let mut writes = Vec::new();
        if let Command::EditClips { clips, edit } = command
            && shared(edit)
        {
            let mut sources = BTreeSet::new();
            clips.retain(|clip| {
                project
                    .clip_alias(*clip)
                    .is_none_or(|source| sources.insert(source))
            });
            for clip in clips.iter().copied() {
                if let Some(source) = project.clip_alias(clip) {
                    writes.push(Write {
                        source,
                        clip,
                        command: edit.command(
                            project,
                            clip,
                            project.midi_source(clip).is_some(),
                        )?,
                        before_audio: project.clip(clip).cloned(),
                        prefix: BeatTime::ZERO,
                    });
                }
            }
        } else {
            let clip = match command.clip_scope() {
                ClipCommandScope::Property { clip, edit } if shared(&edit) => Some(clip),
                ClipCommandScope::FocusedSource { clip }
                | ClipCommandScope::FocusedSnapshot { clip } => Some(clip),
                ClipCommandScope::FocusedNote { note } => {
                    crate::midi::find_note_source(project, note).map(|(clip, _)| clip)
                }
                ClipCommandScope::Geometry { clip }
                    if matches!(
                        command,
                        Command::TrimMidiClipEdge { .. }
                            | Command::StretchAudioClipEdge { .. }
                            | Command::StretchMidiClipEdge { .. }
                            | Command::ResizeMidiClipView { .. }
                    ) =>
                {
                    Some(clip)
                }
                _ => None,
            };
            if let Some(clip) = clip
                && let Some(source) = project.clip_alias(clip)
            {
                let prefix = if let Command::TrimMidiClipEdge { edge, beat, .. } = command {
                    project
                        .midi_clip(clip)
                        .ok_or(Invalid("alias trim member missing"))?
                        .preview_edge_trim(*edge, *beat)?
                        .source_prefix()
                } else {
                    BeatTime::ZERO
                };
                writes.push(Write {
                    source,
                    clip,
                    command: command.clone(),
                    before_audio: project.clip(clip).cloned(),
                    prefix,
                });
            }
        }
        Ok(Self { writes, explicit })
    }
    pub(crate) fn finish(self, project: &mut Project) -> Result<bool, Invalid> {
        let mut validate = self.explicit || !self.writes.is_empty();
        // Every command reconciles membership, whatever its scope (Pulse
        // #3966): section transforms, overwriting placements and conversions
        // also remove, replace or re-kind members. The capture-time scope only
        // bounds which shared writes are recorded.
        if !project.has_clip_aliases() {
            return Ok(validate);
        }
        let original_members = project.aliases.members.len();
        let allowed: BTreeSet<_> = self.writes.iter().map(|write| write.source).collect();
        // Baking/replacing a placement is independent content. It must not edit
        // siblings merely because an old placement ID survived the operation.
        let detached: Vec<_> = project
            .aliases
            .members
            .iter()
            .filter_map(|(clip, source)| {
                let mismatched = match content(project, *clip) {
                    Some(current) => project.aliases.sources.get(source) != Some(&current),
                    // A plain audio clip that kept a MIDI member's ID (MIDI to
                    // audio conversion) has no comparable content; prune drops
                    // members whose clip is gone.
                    None => project.clip(*clip).is_some(),
                };
                (!allowed.contains(source) && mismatched).then_some(*clip)
            })
            .collect();
        if !detached.is_empty() {
            validate = true;
            crate::clip_alias_commands::unique(project, detached)?;
        }
        crate::clip_alias_commands::prune(project);
        validate |= original_members != project.aliases.members.len();
        for write in self.writes {
            let Some(previous) = project.aliases.sources.get(&write.source).cloned() else {
                continue;
            };
            let next =
                content(project, write.clip).ok_or(Invalid("shared edit lost its source"))?;
            if previous == next && !matches!(write.command, Command::SetAudioClipMusical { .. }) {
                continue;
            }
            let peers: Vec<_> = project
                .aliases
                .members
                .iter()
                .filter_map(|(clip, source)| {
                    (*source == write.source && *clip != write.clip).then_some(*clip)
                })
                .collect();
            for peer in peers {
                let current = content(project, peer).ok_or(Invalid("alias member disappeared"))?;
                if current != previous && current != next {
                    return Err(Invalid("conflicting edits to one alias source"));
                }
                match &next {
                    Content::Midi(content) => {
                        if write.prefix != BeatTime::ZERO {
                            let shift = |beat: BeatTime| {
                                beat.raw()
                                    .checked_add(write.prefix.raw())
                                    .map(BeatTime::from_raw)
                                    .ok_or(Invalid("alias source prefix overflow"))
                            };
                            let volume = project
                                .midi_clip_volume(peer)
                                .ok_or(Invalid("alias midi volume missing"))?
                                .shifted(write.prefix, content.source_length)?;
                            let source = project.midi_source_mut(peer)?;
                            source.source_start = shift(source.source_start)?;
                            source.loop_start = shift(source.loop_start)?;
                            source.loop_end = shift(source.loop_end)?;
                            source.content = Arc::clone(content);
                            project.replace_midi_clip_volume(peer, volume)?;
                        }
                        project.midi_source_mut(peer)?.content = Arc::clone(content);
                    }
                    Content::Audio { asset, content } => {
                        let before = write
                            .before_audio
                            .as_ref()
                            .ok_or(Invalid("alias audio origin missing"))?;
                        let target = project
                            .clip(peer)
                            .cloned()
                            .ok_or(Invalid("alias audio member missing"))?;
                        let replacement = crate::audio_edit::rebase_alias_content(
                            &target,
                            before,
                            content,
                            &write.command,
                            project.tempo_map(),
                        )?;
                        if replacement.asset != *asset {
                            return Err(Invalid("alias audio media changed unexpectedly"));
                        }
                        *project.clip_mut(peer)? = replacement;
                    }
                    Content::ConsoleAudio(_) => {
                        return Err(Invalid(
                            "console source replacement requires an explicit media operation",
                        ));
                    }
                }
            }
            project.aliases.sources.insert(write.source, next);
        }
        let aliases = std::mem::take(&mut project.aliases);
        let result = aliases.validate_and_bind(project);
        project.aliases = aliases;
        result.map(|()| validate)
    }
}
