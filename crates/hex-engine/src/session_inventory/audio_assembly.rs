//! Sealed control-only final Session targets. No public inventory/clip reference,
//! Arc, Deref, raw adoption or mutable graph escape is provided by this package.
use super::*;
use crate::{
    AudioStorageError,
    audio_only_inputs::{OwnedAudioInputs, OwnedAudioSource, SourceKind},
    owned_shared::{OwnedShared, SharedDeclaration},
    session_audio_preparation::SessionSourceConstruction,
    session_shared_preparation::SessionSharedConstruction,
    session_structure_preparation::SessionStructureConstruction,
    session_tempo_preparation::SessionTempoConstruction,
};
use std::mem;
mod cleanup;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionAssemblyCapacity {
    pub scene_bytes: usize,
    pub track_bytes: usize,
    pub cell_bytes: usize,
    pub index_bytes: usize,
    pub range_bytes: usize,
    pub audio_bytes: usize,
    pub pair_bytes: usize,
    pub previous_growth_backing_bytes: usize,
}
impl SessionAssemblyCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        [
            self.scene_bytes,
            self.track_bytes,
            self.cell_bytes,
            self.index_bytes,
            self.range_bytes,
            self.audio_bytes,
            self.pair_bytes,
        ]
        .into_iter()
        .try_fold(0_usize, |n, b| {
            n.checked_add(b).ok_or(AudioStorageError::Overflow)
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipPlan {
    pub(crate) source_row: usize,
    pub(crate) bank_source: usize,
    pub(crate) handle: u64,
    pub(crate) clip: ClipId,
    pub(crate) track: TrackId,
    pub(crate) scene: SceneId,
    pub(crate) asset: AssetId,
    pub(crate) start: u64,
    pub(crate) end: u64,
}
pub(crate) struct SceneRangePlan {
    pub(crate) id: SceneId,
    pub(crate) start: usize,
    pub(crate) end: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Step {
    #[default]
    Unstarted,
    Pending,
    Complete,
}
/// Whole final-order row: binding and declaration cannot be independently sorted.
/// Constructor declaration is non-Clone and owns no extra Arc/Weak handle.
pub(crate) struct AudioTarget {
    plan: ClipPlan,
    clip: Option<PreparedSessionAudioClip>,
    declaration: Option<SharedDeclaration>,
    actual_handle: Option<u64>,
    source_disposal: Step,
    name_disposal: Step,
}
impl AudioTarget {
    pub(crate) fn vacant(&self) -> bool {
        self.clip.is_none() && self.actual_handle.is_none() && self.declaration.is_none()
    }
    pub(crate) fn commit(&mut self, owner: OwnedAudioSource, name: String) {
        let OwnedAudioSource { kind, handle } = owner;
        let source = match kind {
            SourceKind::Resident(audio) => PreparedAudioSource::Resident(audio),
            SourceKind::Streamed(metadata) => {
                let (arc, declaration) = metadata.into_paired_parts_on_control();
                self.declaration = Some(declaration);
                PreparedAudioSource::Streamed(arc)
            }
        };
        // No fallible call/destructor between source split and persistent install.
        self.clip = Some(PreparedSessionAudioClip {
            id: self.plan.clip,
            track: self.plan.track,
            scene: self.plan.scene,
            name,
            asset: self.plan.asset,
            source_start: self.plan.start,
            source_end: self.plan.end,
            source,
        });
        self.actual_handle = Some(handle);
    }
    #[cfg(test)]
    fn matches(
        &self,
        clip: &PreparedSessionAudioClip,
        bank: &OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        let (asset, source) = bank
            .sources
            .get(self.plan.bank_source)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.actual_handle != Some(self.plan.handle)
            || *asset != self.plan.asset
            || clip.id != self.plan.clip
            || clip.track != self.plan.track
            || clip.scene != self.plan.scene
            || clip.asset != self.plan.asset
            || clip.source_start != self.plan.start
            || clip.source_end != self.plan.end
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let same = match (&clip.source, &source.kind) {
            (PreparedAudioSource::Resident(a), SourceKind::Resident(b)) => {
                self.declaration.is_none() && a.shares_pcm_with(b)
            }
            (PreparedAudioSource::Streamed(a), SourceKind::Streamed(b)) => {
                Arc::ptr_eq(a, b.arc())
                    && self.declaration.as_ref().and_then(|d| d.parts(a)) == b.parts()
                    && b.parts().is_some()
            }
            _ => false,
        };
        if !same {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AssemblyPhase {
    #[default]
    Vacant,
    Reserved,
    Partial,
    Installed,
    Cleaning,
    Complete,
}
#[derive(Default)]
pub(crate) struct SessionAssembly {
    scenes: Vec<PreparedSessionScene>,
    tracks: Vec<PreparedSessionTrack>,
    cells: Vec<PreparedSessionCell>,
    indices: Vec<usize>,
    ranges: Vec<PreparedSessionSceneCellRange>,
    audio: Vec<PreparedSessionAudioClip>,
    pairs: Vec<AudioTarget>,
    inventory: Option<PreparedSessionInventory>,
    timing: Option<PreparedTempoMap>,
    sine: Option<Arc<SineTable>>,
    sine_declaration: Option<SharedDeclaration>,
    contexts: Option<OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>>,
    metadata_handles: [u64; 3],
    phase: AssemblyPhase,
    growth: usize,
    unused_midi: Vec<PreparedSessionMidiClip>,
    unused_mutable: Vec<PreparedMutableTrackInstrument>,
    name_pending: bool,
    scene_cursor: usize,
    track_cursor: usize,
    pending_source: Option<PreparedAudioSource>,
    pending_name: Option<String>,
    pending_pair: Option<usize>,
    sine_disposal: Step,
    context_disposal: Step,
    #[cfg(test)]
    pub(crate) fail_after_move: Option<usize>,
    #[cfg(test)]
    pub(crate) wrong_handle: bool,
    #[cfg(test)]
    pub(crate) occupy_name: bool,
    #[cfg(test)]
    pub(crate) panic_after_move: Option<usize>,
    #[cfg(test)]
    pub(crate) disposal_probe: Option<crate::audio_preparation::DropProbe>,
}
fn bytes<T>(v: &Vec<T>) -> Result<usize, AudioStorageError> {
    v.capacity()
        .checked_mul(mem::size_of::<T>())
        .ok_or(AudioStorageError::Overflow)
}
fn reserve<T>(v: &mut Vec<T>, n: usize, growth: &mut usize) -> Result<(), AudioStorageError> {
    if n > v.capacity() {
        *growth = growth
            .checked_add(bytes(v)?)
            .ok_or(AudioStorageError::Overflow)?;
    }
    v.try_reserve_exact(n.saturating_sub(v.len()))
        .map_err(|_| AudioStorageError::Allocation)
}
impl SessionAssembly {
    pub(crate) fn shared_storage(
        &self,
    ) -> Result<crate::session_shared_preparation::InventorySharedView<'_>, AudioStorageError> {
        let (sine, contexts) = if let Some(i) = &self.inventory {
            if self.sine.is_some() || self.contexts.is_some() {
                return Err(AudioStorageError::SourceMismatch);
            }
            (Some(&i.sine_table), Some(&i.shared_contexts))
        } else {
            (self.sine.as_ref(), self.contexts.as_ref())
        };
        let sine = match (sine, self.sine_declaration.as_ref()) {
            (Some(a), Some(d)) => Some((a, d)),
            (None, None) => None,
            _ => return Err(AudioStorageError::SourceMismatch),
        };
        Ok(crate::session_shared_preparation::InventorySharedView { sine, contexts })
    }
    pub(crate) fn capacity(
        &self,
        include_growth: bool,
    ) -> Result<SessionAssemblyCapacity, AudioStorageError> {
        // Exhaustive ownership inventory: seven charged Vec backings; names,
        // referenced source allocations, tempo and fixed shared data stay in
        // their parent charge groups while their exact paired owning locations
        // move. Optional inventory contains those SAME vectors, never duplicates.
        // Pending name/source and timing/shared destinations are persistent.
        if let Some(i) = &self.inventory {
            if self.scenes.capacity() != 0
                || self.tracks.capacity() != 0
                || self.cells.capacity() != 0
                || self.indices.capacity() != 0
                || self.ranges.capacity() != 0
                || self.audio.capacity() != 0
                || !i.midi_clips.is_empty()
                || i.midi_clips.capacity() != 0
                || !i.mutable_instruments.is_empty()
                || i.mutable_instruments.capacity() != 0
                || i.mutable_history_retarget_storage_bytes != 0
                || i.midi_note_bank_storage_bytes != 0
                || i.midi_note_bank_reference_count != 0
                || i.work != SessionWorkEnvelope::default()
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        if self.unused_midi.capacity() != 0 || self.unused_mutable.capacity() != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        // Installed vectors have moved to inventory; count actual backings there.
        let (scenes, tracks, cells, indices, ranges, audio) = if let Some(i) = &self.inventory {
            (
                &i.scenes,
                &i.tracks,
                &i.cells,
                &i.scene_cell_indices,
                &i.scene_cell_ranges,
                &i.audio_clips,
            )
        } else {
            (
                &self.scenes,
                &self.tracks,
                &self.cells,
                &self.indices,
                &self.ranges,
                &self.audio,
            )
        };
        let report = SessionAssemblyCapacity {
            scene_bytes: bytes(scenes)?,
            track_bytes: bytes(tracks)?,
            cell_bytes: bytes(cells)?,
            index_bytes: bytes(indices)?,
            range_bytes: bytes(ranges)?,
            audio_bytes: bytes(audio)?,
            pair_bytes: bytes(&self.pairs)?,
            previous_growth_backing_bytes: if include_growth { self.growth } else { 0 },
        };
        report.bytes()?;
        Ok(report)
    }
    pub(crate) fn reserve(
        &mut self,
        structure: &SessionStructureConstruction,
        sources: &SessionSourceConstruction,
        handles: [u64; 3],
    ) -> Result<(), AudioStorageError> {
        if self.phase != AssemblyPhase::Vacant
            || self.inventory.is_some()
            || !self.pairs.is_empty()
            || !self.scenes.is_empty()
            || !self.tracks.is_empty()
            || !self.cells.is_empty()
            || !self.audio.is_empty()
            || !self.indices.is_empty()
            || !self.ranges.is_empty()
            || self.pending_pair.is_some()
            || self.pending_name.is_some()
            || self.pending_source.is_some()
            || self.scene_cursor != 0
            || self.track_cursor != 0
            || self.name_pending
            || self.unused_midi.capacity() != 0
            || self.unused_mutable.capacity() != 0
            || self.sine_declaration.is_some()
            || self.sine.is_some()
            || self.contexts.is_some()
            || self.timing.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        self.phase = AssemblyPhase::Reserved;
        self.growth = 0;
        let (scenes, tracks, cells) = structure.counts();
        let audio = sources.assembly_plans().count();
        reserve(&mut self.scenes, scenes, &mut self.growth)?;
        reserve(&mut self.tracks, tracks, &mut self.growth)?;
        reserve(&mut self.cells, cells, &mut self.growth)?;
        reserve(&mut self.indices, cells, &mut self.growth)?;
        reserve(&mut self.ranges, scenes, &mut self.growth)?;
        reserve(&mut self.audio, audio, &mut self.growth)?;
        reserve(&mut self.pairs, audio, &mut self.growth)?;
        self.scenes.extend(structure.assembly_scene_targets());
        self.tracks.extend(structure.assembly_track_targets());
        self.cells.extend(structure.assembly_cells(sources)?);
        self.indices.extend_from_slice(structure.assembly_indices());
        self.ranges.extend(
            structure
                .assembly_ranges()
                .map(|r| PreparedSessionSceneCellRange {
                    id: r.id,
                    start: r.start,
                    end: r.end,
                }),
        );
        self.pairs
            .extend(sources.assembly_plans().map(|plan| AudioTarget {
                plan,
                clip: None,
                declaration: None,
                actual_handle: None,
                source_disposal: Step::Unstarted,
                name_disposal: Step::Unstarted,
            }));
        self.pairs.sort_unstable_by_key(|r| r.plan.clip);
        if self
            .pairs
            .windows(2)
            .any(|w| w[0].plan.clip >= w[1].plan.clip)
            || handles.contains(&0)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        #[cfg(test)]
        {
            if self.wrong_handle {
                if let Some(row) = self.pairs.first_mut() {
                    row.plan.handle = row
                        .plan
                        .handle
                        .checked_add(1)
                        .ok_or(AudioStorageError::Overflow)?;
                }
            }
            if self.occupy_name {
                if let Some(scene) = self.scenes.first_mut() {
                    scene.name = "occupied negative owner".into();
                }
            }
        }
        self.metadata_handles = handles;
        Ok(())
    }
    pub(crate) fn construct(
        &mut self,
        sources: &mut SessionSourceConstruction,
        structure: &mut SessionStructureConstruction,
        tempo: &mut SessionTempoConstruction,
        shared: &mut SessionSharedConstruction,
        bank: &OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        if self.phase != AssemblyPhase::Reserved
            || self.inventory.is_some()
            || !self.audio.is_empty()
            || self.audio.capacity() < self.pairs.len()
            || self.timing.is_some()
            || self.sine.is_some()
            || self.sine_declaration.is_some()
            || self.contexts.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        structure.validate(sources, true)?;
        structure.preflight_final_names(&self.scenes, &self.tracks)?;
        for row in &self.pairs {
            if !row.vacant() {
                return Err(AudioStorageError::Capacity);
            }
            sources.validate_assembly_row(&row.plan, bank)?;
        }
        shared.preflight_inventory_transfer(self.metadata_handles)?;
        tempo.preflight_inventory_transfer()?;
        self.phase = AssemblyPhase::Partial;
        #[cfg(test)]
        let mut moved = 0;
        for index in 0..self.scenes.len() {
            if !structure.commit_scene(index, &mut self.scenes) {
                return Err(AudioStorageError::SourceMismatch);
            }
            #[cfg(test)]
            {
                self.fault(moved)?;
                moved += 1;
            }
        }
        for index in 0..self.tracks.len() {
            if !structure.commit_track(index, &mut self.tracks) {
                return Err(AudioStorageError::SourceMismatch);
            }
            #[cfg(test)]
            {
                self.fault(moved)?;
                moved += 1;
            }
        }
        for index in 0..self.pairs.len() {
            let row = &mut self.pairs[index];
            let plan = row.plan;
            if !sources.commit_assembly_row(&plan, row) {
                return Err(AudioStorageError::SourceMismatch);
            }
            #[cfg(test)]
            {
                self.fault(moved)?;
                moved += 1;
            }
        }
        // Fixed moves from already paired persistent rows to final sorted vector.
        for row in &mut self.pairs {
            if let Some(clip) = row.clip.take() {
                self.audio.push(clip);
            } else {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        if !tempo.commit_inventory_transfer(&mut self.timing)
            || !shared.commit_inventory_transfer(
                &mut self.sine,
                &mut self.sine_declaration,
                &mut self.contexts,
            )
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        #[cfg(test)]
        {
            self.fault(moved)?;
        }
        // Test audit observes the actual installed tuple/allocation identity.
        // Normal commit already borrowed-preflighted every source/target; no
        // new fallible provider/allocator work occurs after those fixed moves.
        #[cfg(test)]
        self.validate_pairs(bank)?;
        let contains_streamed_sources = self
            .audio
            .iter()
            .any(|c| matches!(c.source, PreparedAudioSource::Streamed(_)));
        if self.timing.is_none() || self.sine.is_none() || self.contexts.is_none() {
            return Err(AudioStorageError::UnknownStorage);
        }
        // No allocation/fallible helper after taking the first final owner.
        if let (Some(timing), Some(sine_table), Some(shared_contexts)) =
            (self.timing.take(), self.sine.take(), self.contexts.take())
        {
            self.inventory = Some(PreparedSessionInventory {
                timing,
                periodic_tempo: tempo.periodic,
                periodic_tempo_error: tempo.periodic_error,
                source_admission_max_micro_bpm: tempo.source_admission_max_micro_bpm,
                sine_table,
                scenes: mem::take(&mut self.scenes),
                cells: mem::take(&mut self.cells),
                scene_cell_indices: mem::take(&mut self.indices),
                scene_cell_ranges: mem::take(&mut self.ranges),
                tracks: mem::take(&mut self.tracks),
                midi_clips: Vec::new(),
                mutable_instruments: Vec::new(),
                mutable_history_retarget_storage_bytes: 0,
                audio_clips: mem::take(&mut self.audio),
                work: SessionWorkEnvelope::default(),
                contains_streamed_sources,
                midi_note_bank_storage_bytes: 0,
                midi_note_bank_reference_count: 0,
                shared_contexts,
            });
            self.phase = AssemblyPhase::Installed;
            #[cfg(test)]
            self.fault(moved + 1)?;
            Ok(())
        } else {
            Err(AudioStorageError::UnknownStorage)
        }
    }
    #[cfg(test)]
    pub(crate) fn validate_pairs(&self, bank: &OwnedAudioInputs) -> Result<(), AudioStorageError> {
        let clips = self
            .inventory
            .as_ref()
            .map_or(self.audio.as_slice(), |i| i.audio_clips.as_slice());
        if clips.len() != self.pairs.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (row, clip) in self.pairs.iter().zip(clips) {
            row.matches(clip, bank)?;
        }
        Ok(())
    }
    #[cfg(test)]
    fn fault(&self, index: usize) -> Result<(), AudioStorageError> {
        assert_ne!(
            self.panic_after_move,
            Some(index),
            "persistent final Session assembly panic"
        );
        if self.fail_after_move == Some(index) {
            return Err(AudioStorageError::Allocation);
        }
        Ok(())
    }
    pub(crate) fn installed_clip_handles(&self) -> usize {
        self.inventory
            .as_ref()
            .map_or(self.audio.len(), |i| i.audio_clips.len())
            + self.pairs.iter().filter(|p| p.clip.is_some()).count()
            + usize::from(self.pending_source.is_some())
    }
    #[cfg(test)]
    pub(crate) fn cell_order(&self) -> Vec<(TrackId, SceneId, PreparedSessionCellContent)> {
        self.inventory.as_ref().map_or_else(Vec::new, |i| {
            i.cells
                .iter()
                .map(|c| (c.track, c.scene, c.content))
                .collect()
        })
    }
    #[cfg(test)]
    pub(crate) fn actual_clip_handles(&self) -> Vec<u64> {
        self.pairs.iter().filter_map(|r| r.actual_handle).collect()
    }
    #[cfg(test)]
    pub(crate) fn final_names_bytes(&self) -> usize {
        let (scenes, tracks, audio) = if let Some(i) = &self.inventory {
            (&i.scenes, &i.tracks, &i.audio_clips)
        } else {
            (&self.scenes, &self.tracks, &self.audio)
        };
        scenes.iter().map(|r| r.name.capacity()).sum::<usize>()
            + tracks.iter().map(|r| r.name.capacity()).sum::<usize>()
            + audio.iter().map(|r| r.name.capacity()).sum::<usize>()
            + self
                .pairs
                .iter()
                .filter_map(|r| r.clip.as_ref())
                .map(|r| r.name.capacity())
                .sum::<usize>()
            + self.pending_name.as_ref().map_or(0, |n| n.capacity())
    }
    #[cfg(test)]
    pub(crate) fn clip_data_alias(&self) -> bool {
        let Some(i) = &self.inventory else {
            return false;
        };
        let [first, second, ..] = i.audio_clips.as_slice() else {
            return false;
        };
        match (&first.source, &second.source) {
            (PreparedAudioSource::Resident(a), PreparedAudioSource::Resident(b)) => {
                a.shares_pcm_with(b)
            }
            (PreparedAudioSource::Streamed(a), PreparedAudioSource::Streamed(b)) => {
                Arc::ptr_eq(a, b)
            }
            _ => false,
        }
    }
    #[cfg(test)]
    pub(crate) fn original_clip_handles(&self) -> Vec<u64> {
        self.pairs.iter().map(|r| r.plan.handle).collect()
    }
    #[cfg(test)]
    pub(crate) fn sort_changed(&self) -> bool {
        self.pairs
            .iter()
            .enumerate()
            .any(|(index, r)| r.plan.source_row != index)
    }
    #[cfg(test)]
    pub(crate) fn retained_after_disposal_panic(&self) -> bool {
        self.pending_name.is_some()
            && self.pending_source.is_none()
            && self.pending_pair.is_some()
            && self.sine.is_some()
            && self.contexts.is_some()
    }
    pub(crate) fn metadata(&self) -> Option<SessionInventoryMetadata<'_>> {
        if self.phase != AssemblyPhase::Installed {
            return None;
        }
        let inventory = self.inventory.as_ref()?;
        self.sine_declaration
            .as_ref()?
            .parts(&inventory.sine_table)?;
        Some(SessionInventoryMetadata { inventory })
    }
}
/// Restricted leaf metadata. No Clone/Deref/AsRef or legacy child reference.
/// ```compile_fail
/// fn leak(view: hex_engine::SessionInventoryMetadata<'_>) { let _ = view.sine_table(); }
/// ```
/// ```compile_fail
/// fn leak(view: hex_engine::SessionInventoryMetadata<'_>) { let _ = view.audio_clips(); }
/// ```
pub struct SessionInventoryMetadata<'a> {
    inventory: &'a PreparedSessionInventory,
}
impl<'a> SessionInventoryMetadata<'a> {
    pub fn scene_names(&self) -> impl Iterator<Item = (SceneId, &'a str)> + '_ {
        self.inventory
            .scenes
            .iter()
            .map(|s| (s.id, s.name.as_str()))
    }
    pub fn track_ids(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.inventory.tracks.iter().map(|t| t.id)
    }
    pub fn audio_clip_ids(&self) -> impl Iterator<Item = ClipId> + '_ {
        self.inventory.audio_clips.iter().map(|c| c.id)
    }
    pub fn cell(&self, track: TrackId, scene: SceneId) -> Option<PreparedSessionCellContent> {
        self.inventory.cell(track, scene)
    }
    pub fn contains_streamed_sources(&self) -> bool {
        self.inventory.contains_streamed_sources
    }
    pub fn streamed_source(&self, clip: ClipId) -> Option<&'a crate::StreamedSourceMetadata> {
        match &self.inventory.audio_clip(clip)?.source {
            PreparedAudioSource::Streamed(meta) => Some(meta.as_ref()),
            _ => None,
        }
    }
}
