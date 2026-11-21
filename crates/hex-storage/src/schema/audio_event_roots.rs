//! Visit Arrangement edit roots and independently retained audio sources in saved ownership order.
use crate::{Project, StorageError};
use hex_project::{AudioClipEdit, Snapshot, TrackContentSnapshot};
use std::sync::Arc;

fn walk_edit(
    project: &Project,
    edit: &mut AudioClipEdit,
    path: &mut Vec<u64>,
    visit: &mut impl FnMut(&[u64], &mut AudioClipEdit) -> Result<bool, StorageError>,
) -> Result<bool, StorageError> {
    let mut changed = visit(path, edit)?;
    if let Some(events) = &edit.domain.events
        && !events.sources.is_empty()
    {
        let mut roots = None;
        for (index, root) in events.sources.iter().enumerate() {
            let mut snapshot = root.clip.retained_snapshot();
            let Some(child) = &mut snapshot.edit else {
                continue;
            };
            path.push(root.id);
            let child_changed = walk_edit(project, child, path, visit)?;
            path.pop();
            if child_changed {
                let restored = project
                    .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
                    .map_err(|error| StorageError::Invalid(error.0))?;
                let replacement = roots.get_or_insert_with(|| events.sources.clone());
                replacement[index] = restored;
                changed = true;
            }
        }
        if let Some(roots) = roots
            && let Some(events) = Arc::make_mut(&mut edit.domain).events.as_mut()
        {
            Arc::make_mut(events).sources = roots;
        }
    }
    Ok(changed)
}

pub(super) fn walk(
    project: &Project,
    snapshot: &mut Snapshot,
    mut visit: impl FnMut(&[u64], &mut AudioClipEdit) -> Result<bool, StorageError>,
) -> Result<(), StorageError> {
    let mut path = Vec::with_capacity(hex_project::MAX_RETAINED_AUDIO_DEPTH + 1);
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some(edit) = &mut clip.edit {
                path.push(clip.id);
                walk_edit(project, edit, &mut path, &mut visit)?;
                path.pop();
            }
        }
    }
    super::audio_alias_projection::synchronize(snapshot)
}
