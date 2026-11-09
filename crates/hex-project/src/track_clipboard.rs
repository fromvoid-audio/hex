//! Project-local authored track metadata. No PCM, runtime device instances or UI
//! selection is copied. Control owns capture and destruction; paste runs inside
//! the editor's prepare-before-commit transaction and ordinary history boundary.
//! All track/device/clip/note/route births receive fresh global IDs. Immutable
//! audio processing roots remain shared; MIDI sources receive independent notes.
use crate::{
    Asset, AssetId, AudioFeed, ClipId, Created, DeviceId, Invalid, MidiRoute, NoteId, Project,
    RoutingOwner, SessionCell, SessionCellContent, TempoMap, Track, TrackId, TrackSelectionIds,
};
use std::{collections::BTreeMap, sync::Arc};

/// Frozen full-track or setup-only content, valid only in the originating open
/// project. The service clears its clipboard on project replacement. Asset
/// metadata, scene and route dependencies are checked again at paste admission.
/// Setup-only copies retain mixer/gain automation and the complete device chain,
/// including both A/B authored states. Arrangement clips and output mute
/// ranges, and Session cells, are omitted.
#[derive(Clone, Debug)]
pub struct TrackClipboard {
    tempo: Arc<TempoMap>,
    tracks: Vec<Track>,
    cells: Vec<SessionCell>,
    assets: Vec<(AssetId, Asset)>,
    audio: Vec<AudioFeed>,
    midi: Vec<MidiRoute>,
}

impl Project {
    pub fn capture_tracks(
        &self,
        selection: &TrackSelectionIds,
        without_clips: bool,
    ) -> Result<TrackClipboard, Invalid> {
        let expanded = self.expand_track_selection(selection)?;
        let ids = expanded.ids();
        if ids.iter().any(|id| self.track(*id).is_none()) {
            return Err(Invalid("selected track no longer exists"));
        }
        let mut tracks = Vec::new();
        tracks
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve track clipboard"))?;
        for track in self
            .tracks()
            .iter()
            .filter(|track| ids.contains(&track.id()))
        {
            let mut copy = track.clone();
            if without_clips {
                copy.content.audio_clips.clear();
                copy.content.midi_clips.clear();
                copy.group_output_mutes = Default::default();
            }
            tracks.push(copy);
        }
        let cells: Vec<_> = self
            .session_cells()
            .iter()
            .filter(|cell| !without_clips && ids.contains(&cell.track()))
            .cloned()
            .collect();
        let mut asset_ids = std::collections::BTreeSet::new();
        for track in &tracks {
            asset_ids.extend(
                track
                    .clips()
                    .iter()
                    .flat_map(|clip| clip.referenced_assets()),
            );
            for device in track.devices().iter().chain(track.instrument()) {
                asset_ids.extend(
                    device
                        .authored_states()
                        .flat_map(|s| s.asset_references())
                        .map(|r| AssetId(r.asset.value())),
                );
            }
        }
        for cell in &cells {
            if let SessionCellContent::Audio(clip) = cell.content() {
                asset_ids.insert(clip.asset());
            }
        }
        let assets = asset_ids
            .into_iter()
            .map(|id| {
                self.asset(id)
                    .cloned()
                    .map(|asset| (id, asset))
                    .ok_or(Invalid("copied track references missing media"))
            })
            .collect::<Result<_, _>>()?;
        Ok(TrackClipboard {
            tempo: Arc::new(self.tempo_map().clone()),
            tracks,
            cells,
            assets,
            audio: self
                .audio_feeds()
                .iter()
                .copied()
                .filter(|r| {
                    ids.contains(&r.destination())
                        || matches!(r.source(), RoutingOwner::Track(id) if ids.contains(&id))
                })
                .collect(),
            midi: self
                .midi_routes()
                .iter()
                .copied()
                .filter(|r| ids.contains(&r.destination()) || ids.contains(&r.source()))
                .collect(),
        })
    }
}

pub(crate) fn paste(
    project: &mut Project,
    clipboard: &TrackClipboard,
    destination: crate::TrackDestination,
) -> Result<Created, Invalid> {
    if clipboard.tempo.sample_rate() != project.sample_rate() {
        return Err(Invalid("track clipboard belongs to another sample rate"));
    }
    let index = project.track_hierarchy()?.insertion(destination)?;
    if project
        .tracks
        .len()
        .checked_add(clipboard.tracks.len())
        .is_none_or(|n| n > crate::MAX_TRACKS)
    {
        return Err(Invalid("track limit reached"));
    }
    for (id, asset) in &clipboard.assets {
        if project.asset(*id) != Some(asset) {
            return Err(Invalid("copied media no longer matches the project"));
        }
    }
    for cell in &clipboard.cells {
        if !project
            .session
            .scenes
            .iter()
            .any(|scene| scene.id() == cell.scene())
        {
            return Err(Invalid("copied session scene no longer exists"));
        }
    }
    let (first, last) = project.allocate_id_range(clipboard.tracks.len())?;
    let mut remap = BTreeMap::new();
    for (offset, track) in clipboard.tracks.iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| Invalid("track id offset overflow"))?;
        remap.insert(
            track.id(),
            TrackId(
                first
                    .checked_add(offset)
                    .ok_or(Invalid("track id overflow"))?,
            ),
        );
    }
    let mapped = |id: TrackId| remap.get(&id).copied().unwrap_or(id);
    let mut copies = clipboard.tracks.clone();
    let tempo = project.tempo_map().clone();
    for track in &mut copies {
        track.id = mapped(track.id);
        track.parent_group = track
            .parent_group
            .and_then(|parent| remap.get(&parent).copied())
            .or(destination.parent_group);
        let mut device_ids = Vec::new();
        device_ids
            .try_reserve_exact(track.devices.len())
            .map_err(|_| Invalid("could not reserve copied device identities"))?;
        for device in &mut track.devices {
            // Canonical preorder puts each parent before its descendants. Keep
            // those relationships inside this independent copy, never its source.
            let parent = device
                .parent
                .map(|parent| {
                    device_ids
                        .iter()
                        .find_map(|(old, new)| (*old == parent).then_some(*new))
                        .ok_or(Invalid("copied device parent is missing"))
                })
                .transpose()?;
            let id = DeviceId(project.allocate_id()?);
            device_ids.push((device.id, id));
            device.id = id;
            device.parent = parent;
        }
        if let Some(binding) = &track.instrument {
            // Duplicating a track is an explicit independent-copy operation;
            // it receives a new source and device identity, unlike linking.
            track.instrument = Some(project.copy_source_binding(binding, track.id)?);
        }

        for clip in &mut track.content.audio_clips {
            clip.id = ClipId(project.allocate_id()?);
        }
        for clip in &mut track.content.midi_clips {
            clip.id = ClipId(project.allocate_id()?);
            clip.source = copy_source(project, &clip.source)?;
        }
        crate::project_tempo::retime_track(track, &clipboard.tempo, &tempo)?;
    }
    project
        .tracks
        .try_reserve(copies.len())
        .map_err(|_| Invalid("could not reserve pasted tracks"))?;
    for (offset, track) in copies.into_iter().enumerate() {
        project.tracks.insert(index + offset, track);
    }
    project
        .session
        .cells
        .try_reserve(clipboard.cells.len())
        .map_err(|_| Invalid("could not reserve pasted session cells"))?;
    for original in &clipboard.cells {
        let mut cell = original.clone();
        cell.track = mapped(cell.track);
        match &mut cell.content {
            SessionCellContent::Audio(clip) => clip.id = ClipId(project.allocate_id()?),
            SessionCellContent::Midi(clip) => {
                clip.id = ClipId(project.allocate_id()?);
                clip.source = copy_source(project, &clip.source)?;
            }
            SessionCellContent::Stop => {}
        }
        project.session.cells.push(cell);
    }
    for route in &clipboard.audio {
        let source = match route.source() {
            RoutingOwner::Track(id) => RoutingOwner::Track(mapped(id)),
            other => other,
        };
        project.set_audio_feed(source, mapped(route.destination()), route.gain())?;
    }
    for route in &clipboard.midi {
        // Incoming MIDI is destination-owned. Never replace an outside track's
        // selected bus during duplication. A cut can restore a now-orphaned link
        // only while its original source is absent and destination unassigned.
        let owns_destination = remap.contains_key(&route.destination());
        let restore_cut = project.track(route.source()).is_none()
            && project.track(route.destination()).is_some()
            && !project
                .midi_routes()
                .iter()
                .any(|r| r.destination() == route.destination());
        if owns_destination || restore_cut {
            project.set_midi_source(mapped(route.destination()), Some(mapped(route.source())))?;
        }
    }
    // Includes sends, IDs, device assets, occupancy, counts, session and route
    // invariants. A refusal is discarded by the enclosing editor transaction.
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::Tracks {
        first: TrackId(first),
        last: TrackId(last),
    })
}

fn copy_source(
    project: &mut Project,
    original: &crate::MidiSource,
) -> Result<Arc<crate::MidiSource>, Invalid> {
    let mut source = original.clone();
    for note in &mut source.notes {
        note.id = NoteId(project.allocate_id()?);
    }
    Ok(Arc::new(source))
}

#[cfg(test)]
#[path = "track_clipboard_tests.rs"]
mod tests;
