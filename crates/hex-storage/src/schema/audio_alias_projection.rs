//! Rebind shared audio sources after a private event-state projection. Inputs
//! originate in a validated project; every member must still agree on its full
//! projected content and asset. Inconsistent decoded inventories are refused.
use crate::StorageError;
use hex_project::{AliasContentSnapshot, AudioClipContent, Snapshot, TrackContentSnapshot};
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn synchronize(snapshot: &mut Snapshot) -> Result<(), StorageError> {
    if snapshot.aliases.members.is_empty() {
        return Ok(());
    }
    let members: BTreeMap<_, _> = snapshot
        .aliases
        .members
        .iter()
        .map(|member| (member.clip, member.source))
        .collect();
    let mut proposed: BTreeMap<u64, (u64, Arc<AudioClipContent>)> = BTreeMap::new();
    for track in &snapshot.tracks {
        let clips = match &track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            let Some(source) = members.get(&clip.id) else {
                continue;
            };
            let content = &clip
                .edit
                .as_ref()
                .ok_or(StorageError::Invalid("alias audio member has no root"))?
                .domain
                .content;
            if let Some((asset, previous)) = proposed.get(source) {
                if *asset != clip.asset || previous.as_ref() != content.as_ref() {
                    return Err(StorageError::Invalid(
                        "projected alias audio members disagree",
                    ));
                }
            } else {
                proposed.insert(*source, (clip.asset, Arc::clone(content)));
            }
        }
    }
    for source in &mut snapshot.aliases.sources {
        if let AliasContentSnapshot::Audio { asset, content } = &mut source.content {
            let (member_asset, projected) = proposed
                .remove(&source.id)
                .ok_or(StorageError::Invalid("alias audio source has no member"))?;
            if *asset != member_asset {
                return Err(StorageError::Invalid(
                    "alias audio media differs from source",
                ));
            }
            *content = projected;
        }
    }
    if !proposed.is_empty() {
        return Err(StorageError::Invalid(
            "alias audio source kind differs from member",
        ));
    }
    Ok(())
}
