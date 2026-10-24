//! Persistent owned Session structure for the audio-only preparation path.
//! No public graph owner, shared alias, runtime or admission proof is exposed.
use crate::{
    AudioStorageError,
    session_audio_preparation::{SessionAudioBinding, SessionSourceConstruction},
};
use hex_project::{Project, SceneId, SessionCellContent, TrackId};
use std::mem;
mod assembly;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStructureCapacity {
    pub scene_capacity: usize,
    pub scene_bytes: usize,
    pub track_capacity: usize,
    pub track_bytes: usize,
    pub cell_capacity: usize,
    pub cell_bytes: usize,
    pub track_index_capacity: usize,
    pub track_index_bytes: usize,
    pub scene_index_capacity: usize,
    pub scene_index_bytes: usize,
    pub range_capacity: usize,
    pub range_bytes: usize,
    pub name_backing_bytes: usize,
    pub previous_growth_backing_bytes: usize,
    /// Unresolved actual exclusive Vec/String allocations, not a disposal fact.
    pub unresolved_owned_allocations: usize,
}
impl SessionStructureCapacity {
    pub(super) fn bytes(self) -> Result<usize, AudioStorageError> {
        [
            self.scene_bytes,
            self.track_bytes,
            self.cell_bytes,
            self.track_index_bytes,
            self.scene_index_bytes,
            self.range_bytes,
            self.name_backing_bytes,
        ]
        .into_iter()
        .try_fold(0_usize, |n, bytes| {
            n.checked_add(bytes).ok_or(AudioStorageError::Overflow)
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disposal {
    Unstarted,
    Pending,
    Complete,
    Transferred,
}
struct SceneRow {
    value: crate::PreparedSessionScene,
    disposal: Disposal,
    #[cfg(test)]
    probe: Option<crate::audio_preparation::DropProbe>,
}
struct TrackRow {
    value: crate::PreparedSessionTrack,
    disposal: Disposal,
}
struct CellRow {
    track: TrackId,
    scene: SceneId,
    audio: Option<SessionAudioBinding>,
    midi: Option<hex_project::ClipId>,
}
#[derive(Clone, Copy)]
struct SceneRange {
    id: SceneId,
    start: usize,
    end: usize,
}
#[cfg(test)]
pub(super) struct StructureSnapshot<'a> {
    pub(super) scene_names: Vec<&'a str>,
    pub(super) track_names: Vec<&'a str>,
    pub(super) ranges: Vec<(SceneId, usize, usize)>,
    pub(super) stop_cells: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OwnerLocation {
    Scenes,
    Tracks,
    Cells,
    TrackIndex,
    SceneIndex,
    Ranges,
    SceneName(usize),
    TrackName(usize),
}
/// Actual owner anchor plus private charge location, valid only while this
/// immutable control borrow prevents allocation replacement/reuse. No persistent
/// naked address, content identity, or public scalar constructor.
pub(super) struct StructuralOwner<'a> {
    anchor: &'a SessionStructureConstruction,
    location: OwnerLocation,
}
impl StructuralOwner<'_> {
    #[cfg(test)]
    pub(super) fn same_owner(&self, other: &Self) -> bool {
        std::ptr::eq(self.anchor, other.anchor) && self.location == other.location
    }
    pub(super) fn bytes(&self) -> Result<usize, AudioStorageError> {
        match self.location {
            OwnerLocation::Scenes => bytes(&self.anchor.scenes),
            OwnerLocation::Tracks => bytes(&self.anchor.tracks),
            OwnerLocation::Cells => bytes(&self.anchor.cells),
            OwnerLocation::TrackIndex => bytes(&self.anchor.track_index),
            OwnerLocation::SceneIndex => bytes(&self.anchor.scene_index),
            OwnerLocation::Ranges => bytes(&self.anchor.ranges),
            OwnerLocation::SceneName(index) => self
                .anchor
                .scenes
                .get(index)
                .map(|row| row.value.name.capacity())
                .ok_or(AudioStorageError::UnknownStorage),
            OwnerLocation::TrackName(index) => self
                .anchor
                .tracks
                .get(index)
                .map(|row| row.value.name.capacity())
                .ok_or(AudioStorageError::UnknownStorage),
        }
    }
}
#[derive(Default)]
pub(super) struct SessionStructureConstruction {
    scenes: Vec<SceneRow>,
    tracks: Vec<TrackRow>,
    cells: Vec<CellRow>,
    track_index: Vec<(TrackId, usize)>,
    scene_index: Vec<usize>,
    ranges: Vec<SceneRange>,
    scene_disposal: usize,
    track_disposal: usize,
    pub(super) capacity: SessionStructureCapacity,
    #[cfg(test)]
    pub(super) fail_reserve: Option<usize>,
    #[cfg(test)]
    pub(super) fail_name: Option<usize>,
    #[cfg(test)]
    pub(super) extra_name_reserve: usize,
}
fn bytes<T>(values: &Vec<T>) -> Result<usize, AudioStorageError> {
    values
        .capacity()
        .checked_mul(mem::size_of::<T>())
        .ok_or(AudioStorageError::Overflow)
}
fn name_valid(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= hex_project::MAX_NAME_BYTES
        && !name.chars().any(char::is_control)
}
fn grow_overlap<T>(values: &Vec<T>, needed: usize) -> Result<usize, AudioStorageError> {
    if needed > values.capacity() {
        bytes(values)
    } else {
        Ok(0)
    }
}
impl SessionStructureConstruction {
    /// Control-only exclusive-allocation inventory. A visitor error leaves
    /// partial observations: callers must discard the entire sink on Err.
    /// References are anchored to this live immutable borrow, never retained as
    /// naked addresses or used as a lifetime/disposal-completion receipt.
    pub(super) fn visit_owners<'a>(
        &'a self,
        mut visitor: impl FnMut(StructuralOwner<'a>) -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        for (location, size) in [
            (OwnerLocation::Scenes, bytes(&self.scenes)?),
            (OwnerLocation::Tracks, bytes(&self.tracks)?),
            (OwnerLocation::Cells, bytes(&self.cells)?),
            (OwnerLocation::TrackIndex, bytes(&self.track_index)?),
            (OwnerLocation::SceneIndex, bytes(&self.scene_index)?),
            (OwnerLocation::Ranges, bytes(&self.ranges)?),
        ] {
            if size > 0 {
                visitor(StructuralOwner {
                    anchor: self,
                    location,
                })?;
            }
        }
        for (index, row) in self.scenes.iter().enumerate() {
            if row.value.name.capacity() > 0 {
                visitor(StructuralOwner {
                    anchor: self,
                    location: OwnerLocation::SceneName(index),
                })?;
            }
        }
        for (index, row) in self.tracks.iter().enumerate() {
            if row.value.name.capacity() > 0 {
                visitor(StructuralOwner {
                    anchor: self,
                    location: OwnerLocation::TrackName(index),
                })?;
            }
        }
        Ok(())
    }
    pub(super) fn observe(&self) -> Result<SessionStructureCapacity, AudioStorageError> {
        let mut report = SessionStructureCapacity {
            scene_capacity: self.scenes.capacity(),
            scene_bytes: bytes(&self.scenes)?,
            track_capacity: self.tracks.capacity(),
            track_bytes: bytes(&self.tracks)?,
            cell_capacity: self.cells.capacity(),
            cell_bytes: bytes(&self.cells)?,
            track_index_capacity: self.track_index.capacity(),
            track_index_bytes: bytes(&self.track_index)?,
            scene_index_capacity: self.scene_index.capacity(),
            scene_index_bytes: bytes(&self.scene_index)?,
            range_capacity: self.ranges.capacity(),
            range_bytes: bytes(&self.ranges)?,
            ..SessionStructureCapacity::default()
        };
        let mut total = 0_usize;
        self.visit_owners(|owner| {
            // Exclusive allocations have one private owning charge location.
            total = total
                .checked_add(owner.bytes()?)
                .ok_or(AudioStorageError::Overflow)?;
            report.unresolved_owned_allocations = report
                .unresolved_owned_allocations
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
            if matches!(
                owner.location,
                OwnerLocation::SceneName(_) | OwnerLocation::TrackName(_)
            ) {
                report.name_backing_bytes = report
                    .name_backing_bytes
                    .checked_add(owner.bytes()?)
                    .ok_or(AudioStorageError::Overflow)?;
            }
            Ok(())
        })?;
        if total != report.bytes()? {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(report)
    }
    fn before_reserve(&self, step: usize) -> Result<(), AudioStorageError> {
        #[cfg(test)]
        if self.fail_reserve == Some(step) {
            return Err(AudioStorageError::Allocation);
        }
        let _ = step;
        Ok(())
    }
    /// All rows/names live here before any fallible helper. No stack partial
    /// owning Vec, String clone or prepared-inventory Arc is produced.
    pub(super) fn reserve(
        &mut self,
        project: &Project,
        sources: &SessionSourceConstruction,
    ) -> Result<(), AudioStorageError> {
        self.reserve_scoped(project, sources, false)
    }
    pub(crate) fn reserve_current_sampler(
        &mut self,
        project: &Project,
        sources: &SessionSourceConstruction,
    ) -> Result<(), AudioStorageError> {
        self.reserve_scoped(project, sources, true)
    }
    fn reserve_scoped(
        &mut self,
        project: &Project,
        sources: &SessionSourceConstruction,
        current_sampler: bool,
    ) -> Result<(), AudioStorageError> {
        if !self.scenes.is_empty()
            || !self.tracks.is_empty()
            || !self.cells.is_empty()
            || !self.track_index.is_empty()
            || !self.scene_index.is_empty()
            || !self.ranges.is_empty()
        {
            return Err(AudioStorageError::Capacity);
        }
        let growth = [
            grow_overlap(&self.scenes, project.scenes().len())?,
            grow_overlap(&self.tracks, project.tracks().len())?,
            grow_overlap(&self.cells, project.session_cells().len())?,
            grow_overlap(&self.track_index, project.tracks().len())?,
            grow_overlap(&self.scene_index, project.session_cells().len())?,
            grow_overlap(&self.ranges, project.scenes().len())?,
        ]
        .into_iter()
        .try_fold(0_usize, |n, b| {
            n.checked_add(b).ok_or(AudioStorageError::Overflow)
        })?;
        self.capacity = self.observe()?;
        self.capacity.previous_growth_backing_bytes = growth;
        self.before_reserve(0)?;
        self.scenes
            .try_reserve_exact(project.scenes().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.before_reserve(1)?;
        self.tracks
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.before_reserve(2)?;
        self.cells
            .try_reserve_exact(project.session_cells().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.before_reserve(3)?;
        self.track_index
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.before_reserve(4)?;
        self.scene_index
            .try_reserve_exact(project.session_cells().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.before_reserve(5)?;
        self.ranges
            .try_reserve_exact(project.scenes().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for scene in project.scenes() {
            if !name_valid(scene.name()) || self.scenes.iter().any(|s| s.value.id == scene.id()) {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            self.scenes.push(SceneRow {
                value: crate::PreparedSessionScene {
                    id: scene.id(),
                    name: String::new(),
                },
                disposal: Disposal::Unstarted,
                #[cfg(test)]
                probe: None,
            });
            let index = self.scenes.len() - 1;
            #[cfg(test)]
            if self.fail_name == Some(index) {
                return Err(AudioStorageError::Allocation);
            }
            let requested = scene.name().len();
            #[cfg(test)]
            let requested = requested
                .checked_add(self.extra_name_reserve)
                .ok_or(AudioStorageError::Overflow)?;
            self.scenes[index]
                .value
                .name
                .try_reserve_exact(requested)
                .map_err(|_| AudioStorageError::Allocation)?;
            self.scenes[index].value.name.push_str(scene.name());
        }
        for track in project.tracks() {
            if !name_valid(track.name())
                || !track.gain().is_finite()
                || !track.balance().is_finite()
                || self.tracks.iter().any(|t| t.value.id == track.id())
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            let index = self.tracks.len();
            self.tracks.push(TrackRow {
                value: crate::PreparedSessionTrack {
                    id: track.id(),
                    name: String::new(),
                    gain: track.gain(),
                    balance: track.balance(),
                    muted: track.muted(),
                    soloed: track.soloed(),
                    work: crate::SessionWorkEnvelope::default(),
                },
                disposal: Disposal::Unstarted,
            });
            self.track_index.push((track.id(), index));
            #[cfg(test)]
            if self.fail_name == Some(self.scenes.len() + index) {
                return Err(AudioStorageError::Allocation);
            }
            let requested = track.name().len();
            #[cfg(test)]
            let requested = requested
                .checked_add(self.extra_name_reserve)
                .ok_or(AudioStorageError::Overflow)?;
            self.tracks[index]
                .value
                .name
                .try_reserve_exact(requested)
                .map_err(|_| AudioStorageError::Allocation)?;
            self.tracks[index].value.name.push_str(track.name());
        }
        self.track_index.sort_unstable_by_key(|&(id, _)| id);
        for cell in project.session_cells() {
            if !self.tracks.iter().any(|t| t.value.id == cell.track())
                || !self.scenes.iter().any(|s| s.value.id == cell.scene())
                || self
                    .cells
                    .iter()
                    .any(|c| c.track == cell.track() && c.scene == cell.scene())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            let audio = match cell.content() {
                SessionCellContent::Stop => None,
                SessionCellContent::Audio(clip) => {
                    Some(sources.binding_for(cell.track(), cell.scene(), clip.id())?)
                }
                SessionCellContent::Midi(_) if current_sampler => None,
                SessionCellContent::Midi(_) => return Err(AudioStorageError::UnsupportedGraph),
            };
            self.cells.push(CellRow {
                track: cell.track(),
                scene: cell.scene(),
                audio,
                midi: match cell.content() {
                    SessionCellContent::Midi(clip) => Some(clip.id()),
                    _ => None,
                },
            });
        }
        self.scene_index.extend(0..self.cells.len());
        self.scene_index
            .sort_unstable_by_key(|&index| (self.cells[index].scene, index));
        for scene in &self.scenes {
            self.ranges.push(SceneRange {
                id: scene.value.id,
                start: 0,
                end: 0,
            });
        }
        self.ranges.sort_unstable_by_key(|r| r.id);
        let mut position = 0_usize;
        for range in &mut self.ranges {
            range.start = position;
            while self
                .scene_index
                .get(position)
                .is_some_and(|&i| self.cells[i].scene == range.id)
            {
                position += 1;
            }
            range.end = position;
        }
        if position != self.cells.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.capacity = self.observe()?;
        self.capacity.previous_growth_backing_bytes = growth;
        self.validate(sources, false)
    }
    pub(super) fn validate(
        &self,
        sources: &SessionSourceConstruction,
        installed: bool,
    ) -> Result<(), AudioStorageError> {
        if self.track_index.len() != self.tracks.len()
            || self.scene_index.len() != self.cells.len()
            || self.ranges.len() != self.scenes.len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (position, &(id, index)) in self.track_index.iter().enumerate() {
            if self.tracks.get(index).is_none_or(|t| t.value.id != id) {
                return Err(AudioStorageError::SourceMismatch);
            }
            if position > 0 && self.track_index[position - 1].0 >= id {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for cell in &self.cells {
            if cell
                .audio
                .as_ref()
                .is_some_and(|b| !sources.binding_matches(b, cell.track, cell.scene, installed))
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for (position, &index) in self.scene_index.iter().enumerate() {
            if index >= self.cells.len() || self.scene_index[..position].contains(&index) {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        let mut boundary = 0;
        for (position, range) in self.ranges.iter().enumerate() {
            if range.start != boundary
                || range.start > range.end
                || range.end > self.scene_index.len()
                || !self.scenes.iter().any(|s| s.value.id == range.id)
                || (position > 0 && self.ranges[position - 1].id >= range.id)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            if self.scene_index[range.start..range.end]
                .iter()
                .any(|&i| self.cells.get(i).is_none_or(|c| c.scene != range.id))
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            boundary = range.end;
        }
        if boundary != self.scene_index.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn has_next_disposal(&self) -> bool {
        self.scene_disposal < self.scenes.len() || self.track_disposal < self.tracks.len()
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        if self.scene_disposal < self.scenes.len() {
            let row = &mut self.scenes[self.scene_disposal];
            if !matches!(row.disposal, Disposal::Unstarted | Disposal::Transferred) {
                return Err(AudioStorageError::UnknownStorage);
            }
            row.disposal = Disposal::Pending;
            drop(mem::take(&mut row.value.name));
            #[cfg(test)]
            drop(row.probe.take());
            row.disposal = Disposal::Complete;
            self.scene_disposal += 1;
        } else if self.track_disposal < self.tracks.len() {
            let row = &mut self.tracks[self.track_disposal];
            if !matches!(row.disposal, Disposal::Unstarted | Disposal::Transferred) {
                return Err(AudioStorageError::UnknownStorage);
            }
            row.disposal = Disposal::Pending;
            drop(mem::take(&mut row.value.name));
            row.disposal = Disposal::Complete;
            self.track_disposal += 1;
        }
        Ok(())
    }
    pub(super) fn clear_disposed_rows(&mut self) -> Result<(), AudioStorageError> {
        if self
            .scenes
            .iter()
            .any(|r| r.disposal != Disposal::Complete || r.value.name.capacity() != 0)
            || self
                .tracks
                .iter()
                .any(|r| r.disposal != Disposal::Complete || r.value.name.capacity() != 0)
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.scenes.clear();
        self.tracks.clear();
        self.cells.clear();
        self.track_index.clear();
        self.scene_index.clear();
        self.ranges.clear();
        self.scene_disposal = 0;
        self.track_disposal = 0;
        Ok(())
    }
    pub(super) fn counts(&self) -> (usize, usize, usize) {
        (self.scenes.len(), self.tracks.len(), self.cells.len())
    }
    #[cfg(test)]
    pub(super) fn snapshot(&self) -> StructureSnapshot<'_> {
        StructureSnapshot {
            scene_names: self.scenes.iter().map(|r| r.value.name.as_str()).collect(),
            track_names: self.tracks.iter().map(|r| r.value.name.as_str()).collect(),
            ranges: self.ranges.iter().map(|r| (r.id, r.start, r.end)).collect(),
            stop_cells: self
                .cells
                .iter()
                .filter(|c| c.audio.is_none() && c.midi.is_none())
                .count(),
        }
    }
    #[cfg(test)]
    pub(super) fn set_disposal_probe(
        &mut self,
        index: usize,
        probe: crate::audio_preparation::DropProbe,
    ) {
        self.scenes[index].probe = Some(probe);
    }
    #[cfg(test)]
    pub(super) fn duplicate_first_index(&mut self) {
        self.scene_index[1] = self.scene_index[0];
    }
}

impl SessionStructureConstruction {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            scenes,
            tracks,
            cells,
            track_index,
            scene_index,
            ranges,
            scene_disposal: _,
            track_disposal: _,
            capacity,
            #[cfg(test)]
                fail_reserve: _,
            #[cfg(test)]
                fail_name: _,
            #[cfg(test)]
                extra_name_reserve: _,
        } = self;
        l.vec(scenes)?;
        l.vec(tracks)?;
        l.vec(cells)?;
        l.vec(track_index)?;
        l.vec(scene_index)?;
        l.vec(ranges)?;
        for SceneRow {
            value,
            disposal: _,
            #[cfg(test)]
                probe: _,
        } in scenes
        {
            l.string(&value.name)?;
        }
        for TrackRow { value, disposal: _ } in tracks {
            l.string(&value.name)?;
        }
        l.growth(capacity.previous_growth_backing_bytes)
    }
}
