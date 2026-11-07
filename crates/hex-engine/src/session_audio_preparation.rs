//! Private preparation cache and Session audio source/name destinations. Every
//! owner belongs to the parent's persistent custody; no previous raw inventory
//! or unreported media map is accepted here. This is not a Session runtime.
use crate::audio_only_inputs::*;
use hex_project::{Project, SessionCellContent};
use std::mem;
mod assembly;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionSourceCapacity {
    pub cell_capacity: usize,
    pub cell_bytes: usize,
    pub cache_capacity: usize,
    pub cache_bytes: usize,
    pub name_backing_bytes: usize,
    pub previous_backing_bytes: usize,
    /// Reserved independent cache/clip obligation destinations, including empty
    /// preflight rows. Installed owning handles are counted separately.
    pub reserved_handle_destinations: usize,
}
impl SessionSourceCapacity {
    pub(super) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.cell_bytes
            .checked_add(self.cache_bytes)
            .and_then(|n| n.checked_add(self.name_backing_bytes))
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposal {
    Unstarted,
    Pending,
    Complete,
    Transferred,
}
struct CacheSlot {
    source: usize,
    handle: u64,
    owner: Option<OwnedAudioSource>,
    disposal: Disposal,
}
struct CellSlot {
    active: bool,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    clip: hex_project::ClipId,
    asset: hex_project::AssetId,
    cache: usize,
    handle: u64,
    start: u64,
    end: u64,
    loop_range: Option<hex_project::SessionAudioLoop>,
    gain: hex_project::ClipGain,
    name: String,
    owner: Option<OwnedAudioSource>,
    disposal: Disposal,
    #[cfg(test)]
    test_index: usize,
    #[cfg(test)]
    probe: Option<super::audio_preparation::DropProbe>,
}
/// Private nonowning reference to exactly one persistent source-handle row.
/// No Clone/public scalar construction or lifetime-closure authority.
pub(super) struct SessionAudioBinding {
    index: usize,
    handle: u64,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    clip: hex_project::ClipId,
}
#[derive(Default)]
pub(super) struct SessionSourceConstruction {
    cells: Vec<CellSlot>,
    cache: Vec<CacheSlot>,
    next_cell_disposal: usize,
    next_cache_disposal: usize,
    pub(super) capacity: SessionSourceCapacity,
    #[cfg(test)]
    pub(super) fail_after_alias: Option<usize>,
    #[cfg(test)]
    pub(super) fail_name: Option<usize>,
    #[cfg(test)]
    pub(super) bad_interval: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after_alias: Option<usize>,
    #[cfg(test)]
    pub(super) fail_name_reserve: Option<usize>,
}
impl SessionAudioBinding {
    pub(crate) fn clip_id(&self) -> hex_project::ClipId {
        self.clip
    }
}
impl SessionSourceConstruction {
    pub(super) fn binding_for(
        &self,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        clip: hex_project::ClipId,
    ) -> Result<SessionAudioBinding, AudioStorageError> {
        let index = self
            .cells
            .iter()
            .position(|row| row.track == track && row.scene == scene && row.clip == clip)
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(SessionAudioBinding {
            index,
            handle: self.cells[index].handle,
            track,
            scene,
            clip,
        })
    }
    pub(super) fn binding_matches(
        &self,
        binding: &SessionAudioBinding,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        installed: bool,
    ) -> bool {
        binding.track == track
            && binding.scene == scene
            && self.cells.get(binding.index).is_some_and(|row| {
                row.handle == binding.handle
                    && row.track == binding.track
                    && row.scene == binding.scene
                    && row.clip == binding.clip
                    && (!installed
                        || row
                            .owner
                            .as_ref()
                            .is_some_and(|owner| owner.handle == binding.handle))
            })
    }
    pub(super) fn retained_capacity(&self) -> Result<SessionSourceCapacity, AudioStorageError> {
        let mut report = SessionSourceCapacity {
            cell_capacity: self.cells.capacity(),
            cell_bytes: self
                .cells
                .capacity()
                .checked_mul(mem::size_of::<CellSlot>())
                .ok_or(AudioStorageError::Overflow)?,
            cache_capacity: self.cache.capacity(),
            cache_bytes: self
                .cache
                .capacity()
                .checked_mul(mem::size_of::<CacheSlot>())
                .ok_or(AudioStorageError::Overflow)?,
            ..SessionSourceCapacity::default()
        };
        report.name_backing_bytes = self.cells.iter().try_fold(0_usize, |n, row| {
            n.checked_add(row.name.capacity())
                .ok_or(AudioStorageError::Overflow)
        })?;
        report.reserved_handle_destinations = self
            .cells
            .len()
            .checked_add(self.cache.len())
            .ok_or(AudioStorageError::Overflow)?;
        Ok(report)
    }
    /// Called under a borrowed parent catch, construction phase already set.
    /// The original bank remains borrowed until all reserves/validation succeed.
    /// Fallible String work writes directly to persistent empty destinations.
    pub(super) fn reserve(
        &mut self,
        project: &Project,
        generation: u64,
        bank: &OwnedAudioInputs,
        arrangement_handles: usize,
    ) -> Result<(), AudioStorageError> {
        self.reserve_scoped(project, generation, bank, arrangement_handles, false)
    }
    pub(crate) fn reserve_current_sampler(
        &mut self,
        project: &Project,
        generation: u64,
        bank: &OwnedAudioInputs,
        arrangement_handles: usize,
        effects: bool,
    ) -> Result<(), AudioStorageError> {
        if !crate::native_device_resources::current_sampler::validate_project_with_effects(
            project, effects,
        )? {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        self.reserve_scoped(project, generation, bank, arrangement_handles, true)
    }
    fn reserve_scoped(
        &mut self,
        project: &Project,
        generation: u64,
        bank: &OwnedAudioInputs,
        arrangement_handles: usize,
        current_sampler: bool,
    ) -> Result<(), AudioStorageError> {
        if !self.cells.is_empty() || !self.cache.is_empty() {
            return Err(AudioStorageError::Capacity);
        }
        self.capacity = SessionSourceCapacity::default();
        let count = project
            .session_cells()
            .iter()
            .try_fold(0_usize, |n, cell| match cell.content() {
                SessionCellContent::Midi(_) if current_sampler => Ok(n),
                SessionCellContent::Midi(_) => Err(AudioStorageError::UnsupportedGraph),
                SessionCellContent::Audio(_) => n.checked_add(1).ok_or(AudioStorageError::Overflow),
                SessionCellContent::Stop => Ok(n),
            })?;
        let previous_cells = if count > self.cells.capacity() {
            self.cells.capacity()
        } else {
            0
        };
        let previous_cache = if count > self.cache.capacity() {
            self.cache.capacity()
        } else {
            0
        };
        self.capacity.previous_backing_bytes = previous_cells
            .checked_mul(mem::size_of::<CellSlot>())
            .and_then(|n| {
                previous_cache
                    .checked_mul(mem::size_of::<CacheSlot>())
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or(AudioStorageError::Overflow)?;
        // Upper cache storage bound reserved before any alias. Actual cache row
        // count is measured below; no additional owner is hidden by data dedup.
        self.cells
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        self.capacity.cell_capacity = self.cells.capacity();
        self.capacity.cell_bytes = self
            .cells
            .capacity()
            .checked_mul(mem::size_of::<CellSlot>())
            .ok_or(AudioStorageError::Overflow)?;
        self.cache
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        self.capacity.cache_capacity = self.cache.capacity();
        self.capacity.cache_bytes = self
            .cache
            .capacity()
            .checked_mul(mem::size_of::<CacheSlot>())
            .ok_or(AudioStorageError::Overflow)?;
        for cell in project.session_cells() {
            let SessionCellContent::Audio(clip) = cell.content() else {
                continue;
            };
            if !project.tracks().iter().any(|t| t.id() == cell.track())
                || !project.scenes().iter().any(|s| s.id() == cell.scene())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            let source = bank
                .sources
                .iter()
                .position(|(id, _)| *id == clip.asset())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let asset = project
                .asset(clip.asset())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let owner = &bank.sources[source].1;
            if owner.sample_rate() != project.sample_rate()
                || owner.frames() != asset.playback_frames()
                || clip.source_start() >= clip.source_end()
                || clip.source_end() > owner.frames()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            if let SourceKind::Streamed(meta) = &owner.kind
                && (meta.key().generation() != generation
                    || meta.key().asset() != asset.id()
                    || meta.source_rate() != asset.source_sample_rate()
                    || meta.source_frames() != asset.source_frames()
                    || meta.conversion_version() != asset.conversion_version())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            if clip.name().trim().is_empty()
                || clip.name().len() > hex_project::MAX_NAME_BYTES
                || clip.name().chars().any(char::is_control)
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            let cache = match self.cache.iter().position(|c| c.source == source) {
                Some(index) => index,
                None => {
                    let index = self.cache.len();
                    self.cache.push(CacheSlot {
                        source,
                        handle: handle()?,
                        owner: None,
                        disposal: Disposal::Unstarted,
                    });
                    self.capacity.reserved_handle_destinations = self
                        .cells
                        .len()
                        .checked_add(self.cache.len())
                        .ok_or(AudioStorageError::Overflow)?;
                    index
                }
            };
            // Only an empty String header is moved. Its backing allocation is
            // reserved directly in the persistent cell before source cloning.
            let index = self.cells.len();
            self.cells.push(CellSlot {
                active: clip.active(),
                track: cell.track(),
                scene: cell.scene(),
                clip: clip.id(),
                asset: clip.asset(),
                cache,
                handle: handle()?,
                start: clip.source_start(),
                end: clip.source_end(),
                loop_range: clip.loop_range(),
                gain: clip.gain(),
                name: String::new(),
                owner: None,
                disposal: Disposal::Unstarted,
                #[cfg(test)]
                test_index: index,
                #[cfg(test)]
                probe: None,
            });
            self.capacity.reserved_handle_destinations = self
                .cells
                .len()
                .checked_add(self.cache.len())
                .ok_or(AudioStorageError::Overflow)?;
            #[cfg(test)]
            if self.fail_name_reserve == Some(index) {
                return Err(AudioStorageError::Allocation);
            }
            self.cells[index]
                .name
                .try_reserve_exact(clip.name().len())
                .map_err(|_| AudioStorageError::Allocation)?;
            self.capacity.name_backing_bytes = self
                .capacity
                .name_backing_bytes
                .checked_add(self.cells[index].name.capacity())
                .ok_or(AudioStorageError::Overflow)?;
            self.cells[index].name.push_str(clip.name()); // pre-reserved, exact UTF-8
        }
        self.capacity.reserved_handle_destinations = self
            .cells
            .len()
            .checked_add(self.cache.len())
            .ok_or(AudioStorageError::Overflow)?;
        let total = bank
            .owning_handle_count()
            .checked_add(arrangement_handles)
            .and_then(|n| n.checked_add(self.capacity.reserved_handle_destinations))
            .ok_or(AudioStorageError::Overflow)?;
        if total > bank.reservation.handle_obligations {
            return Err(AudioStorageError::Capacity);
        }
        Ok(())
    }
    /// No fallible allocation/name/path helper after the first alias. Each
    /// private cache and clip handle is separately reserved and installed.
    pub(super) fn construct(&mut self, bank: &OwnedAudioInputs) -> Result<(), AudioStorageError> {
        for row in &mut self.cache {
            let source = &bank
                .sources
                .get(row.source)
                .ok_or(AudioStorageError::SourceMismatch)?
                .1;
            let kind = clone_kind(&source.kind);
            row.owner = Some(OwnedAudioSource {
                kind,
                handle: row.handle,
            });
        }
        for row in &mut self.cells {
            let cache = self
                .cache
                .get(row.cache)
                .and_then(|c| c.owner.as_ref())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let kind = clone_kind(&cache.kind);
            row.owner = Some(OwnedAudioSource {
                kind,
                handle: row.handle,
            });
            #[cfg(test)]
            {
                let index = row.test_index;
                assert_ne!(
                    self.panic_after_alias,
                    Some(index),
                    "Session panic after installed alias"
                );
                if self.fail_after_alias == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
                if self.bad_interval == Some(index) {
                    row.end = u64::MAX;
                }
                if self.fail_name == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
            }
            if row.start >= row.end || row.end > cache.frames() || row.name.trim().is_empty() {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        Ok(())
    }
    /// Borrowed parent catch surrounds each single persistent destination. If
    /// destruction unwinds, Pending remains and subsequent work cannot retry.
    pub(super) fn has_next_disposal(&self) -> bool {
        self.next_cell_disposal < self.cells.len() || self.next_cache_disposal < self.cache.len()
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        if self.next_cell_disposal < self.cells.len() {
            let row = &mut self.cells[self.next_cell_disposal];
            if !matches!(row.disposal, Disposal::Unstarted | Disposal::Transferred) {
                return Err(AudioStorageError::UnknownStorage);
            }
            row.disposal = Disposal::Pending;
            drop(row.owner.take());
            drop(mem::take(&mut row.name));
            #[cfg(test)]
            drop(row.probe.take());
            row.disposal = Disposal::Complete;
            self.next_cell_disposal += 1;
        } else if self.next_cache_disposal < self.cache.len() {
            let row = &mut self.cache[self.next_cache_disposal];
            if row.disposal != Disposal::Unstarted {
                return Err(AudioStorageError::UnknownStorage);
            }
            row.disposal = Disposal::Pending;
            drop(row.owner.take());
            row.disposal = Disposal::Complete;
            self.next_cache_disposal += 1;
        }
        Ok(())
    }
    pub(super) fn clear_disposed_rows(&mut self) -> Result<(), AudioStorageError> {
        if self
            .cells
            .iter()
            .any(|r| r.disposal != Disposal::Complete || r.owner.is_some() || !r.name.is_empty())
            || self
                .cache
                .iter()
                .any(|r| r.disposal != Disposal::Complete || r.owner.is_some())
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.cells.clear();
        self.cache.clear(); // no live owner/name backing
        self.next_cell_disposal = 0;
        self.next_cache_disposal = 0;
        Ok(())
    }
    pub(super) fn counts(&self) -> (usize, usize) {
        let cells = self
            .cells
            .iter()
            .filter(|row| {
                let _location = (row.track, row.scene, row.clip);
                row.owner.as_ref().is_some_and(|o| o.handle == row.handle)
            })
            .count();
        let cache = self
            .cache
            .iter()
            .filter(|row| row.owner.as_ref().is_some_and(|o| o.handle == row.handle))
            .count();
        (cells, cache)
    }
    #[cfg(test)]
    pub(super) fn set_disposal_probe(
        &mut self,
        index: usize,
        probe: super::audio_preparation::DropProbe,
    ) {
        self.cells[index].probe = Some(probe);
    }
    #[cfg(test)]
    pub(super) fn validate_actual_aliases(&self, bank: &OwnedAudioInputs) -> bool {
        self.cache.iter().all(|row| {
            row.owner.as_ref().is_some_and(|owner| {
                let original = &bank.sources[row.source].1;
                owner.handle == row.handle
                    && owner.handle != original.handle
                    && crate::audio_input_storage::same(owner, original)
            })
        }) && self.cells.iter().all(|row| {
            row.owner.as_ref().is_some_and(|owner| {
                self.cache[row.cache].owner.as_ref().is_some_and(|cache| {
                    owner.handle == row.handle
                        && owner.handle != cache.handle
                        && crate::audio_input_storage::same(owner, cache)
                })
            })
        })
    }
    #[cfg(test)]
    pub(super) fn names(&self) -> impl Iterator<Item = &str> {
        self.cells.iter().map(|row| row.name.as_str())
    }
}
fn clone_kind(kind: &SourceKind) -> SourceKind {
    match kind {
        SourceKind::Resident(audio) => SourceKind::Resident(audio.clone()),
        SourceKind::Streamed(metadata) => SourceKind::Streamed(metadata.clone_on_control()),
    }
}

impl SessionSourceConstruction {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            cells,
            cache,
            next_cell_disposal: _,
            next_cache_disposal: _,
            capacity,
            #[cfg(test)]
                fail_after_alias: _,
            #[cfg(test)]
                fail_name: _,
            #[cfg(test)]
                bad_interval: _,
            #[cfg(test)]
                panic_after_alias: _,
            #[cfg(test)]
                fail_name_reserve: _,
        } = self;
        l.vec(cells)?;
        l.vec(cache)?;
        for row in cells {
            let CellSlot {
                active: _,
                track: _,
                scene: _,
                clip: _,
                asset: _,
                cache: _,
                handle: _,
                start: _,
                end: _,
                loop_range: _,
                gain: _,
                name,
                owner,
                disposal: _,
                #[cfg(test)]
                    test_index: _,
                #[cfg(test)]
                    probe: _,
            } = row;
            l.string(name)?;
            if let Some(source) = owner {
                l.source(source)?;
            }
        }
        for row in cache {
            let CacheSlot {
                source: _,
                handle: _,
                owner,
                disposal: _,
            } = row;
            if let Some(source) = owner {
                l.source(source)?;
            }
        }
        l.growth(capacity.previous_backing_bytes)
    }
}
