//! Private final Arrangement clip owners; not a Track/Project or publication.
//! Every Box and paired destination exists before any source alias moves.
pub(super) mod chains;
#[cfg(test)]
mod preview_tests;
pub(super) mod returns;
mod root;
pub(super) mod tracks;
use super::*;
use crate::{Clip, ClipMetadata, owned_shared::SharedDeclaration};
pub use root::ArrangementRootCapacity;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementClipCapacity {
    pub tracks: tracks::ArrangementTrackCapacity,
    pub root: ArrangementRootCapacity,
    pub returns: returns::ArrangementReturnCapacity,
    pub preview: crate::midi_preview::persistent::AudioPreviewCapacity,
    pub pair_backing_bytes: usize,
    pub metadata_pointee_bytes: usize,
    pub previous_growth_backing_bytes: usize,
}
impl ArrangementClipCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.pair_backing_bytes
            .checked_add(self.metadata_pointee_bytes)
            .and_then(|n| n.checked_add(self.tracks.bytes().ok()?))
            .and_then(|n| n.checked_add(self.root.bytes().ok()?))
            .and_then(|n| n.checked_add(self.returns.bytes().ok()?))
            .and_then(|n| n.checked_add(self.preview.bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding {
    slot: usize,
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    source: usize,
    handle: u64,
}
struct Pair {
    transferred: bool,
    binding: Binding,
    clip: Option<Clip>,
    metadata: Option<Box<ClipMetadata>>,
    declaration: Option<SharedDeclaration>,
    installed_handle: Option<u64>,
    installed_slot: Option<usize>,
    stream: Option<Arc<crate::StreamedSourceMetadata>>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Vacant,
    Reserved,
    Installed,
    Cleaning,
    Complete,
}
#[derive(Default)]
pub(crate) struct ArrangementClips {
    rows: Vec<Pair>,
    tracks: tracks::TrackAssembly,
    root: root::ArrangementRootAssembly,
    returns: returns::ReturnAssembly,
    preview: crate::midi_preview::persistent::PreviewCustody,
    phase: Phase,
    growth: usize,
    cursor: usize,
    pending_audio: Option<crate::Audio>,
    pending_stream: Option<Arc<crate::StreamedSourceMetadata>>,
    pending_metadata: Option<Box<ClipMetadata>>,
    source_disposal: Disposal,
    metadata_disposal: Disposal,
    staged: bool,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) mismatch: bool,
    #[cfg(test)]
    pub(super) probe: Option<DropProbe>,
}
pub(crate) fn sampler_compiler_quote(
    project: &hex_project::Project,
) -> Result<(usize, usize), AudioStorageError> {
    project
        .tracks()
        .iter()
        .filter(|t| t.instrument().is_some())
        .try_fold((0usize, 0usize), |(retained, scratch), track| {
            let (r, s) = tracks::sampler_compiler_quote(track)?;
            Ok((
                retained.checked_add(r).ok_or(AudioStorageError::Overflow)?,
                scratch.max(s),
            ))
        })
}
pub(crate) fn sampler_workspace_bound(
    notes: usize,
    clips: usize,
) -> Result<usize, AudioStorageError> {
    tracks::sampler_workspace_bound(notes, clips)
}
impl ArrangementClips {
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementClipCapacity, AudioStorageError> {
        self.capacity_at(growth, None, None)
    }
    pub(crate) fn capacity_at(
        &self,
        growth: bool,
        project: Option<&crate::Project>,
        preview: Option<&crate::MidiArrangementPreview>,
    ) -> Result<ArrangementClipCapacity, AudioStorageError> {
        let root = self.root.view_at(project)?;
        let metadata =
            self.rows
                .iter()
                .try_fold(usize::from(self.pending_metadata.is_some()), |n, r| {
                    n.checked_add(usize::from(r.metadata.is_some()))
                        .and_then(|n| {
                            n.checked_add(usize::from(
                                r.clip.as_ref().is_some_and(|c| c.metadata.is_some()),
                            ))
                        })
                        .ok_or(AudioStorageError::Overflow)
                })?;
        Ok(ArrangementClipCapacity {
            tracks: self.tracks.capacity_at(growth, root.tracks)?,
            root: self.root.capacity_at(growth, project)?,
            returns: self.returns.capacity_at(growth, root.returns, root.feed)?,
            preview: self.preview.capacity_at(preview)?,
            pair_backing_bytes: self
                .rows
                .capacity()
                .checked_mul(mem::size_of::<Pair>())
                .ok_or(AudioStorageError::Overflow)?,
            metadata_pointee_bytes: metadata
                .checked_add(self.tracks.metadata_boxes(root.tracks)?)
                .ok_or(AudioStorageError::Overflow)?
                .checked_mul(mem::size_of::<ClipMetadata>())
                .ok_or(AudioStorageError::Overflow)?,
            previous_growth_backing_bytes: if growth {
                self.growth
                    .checked_add(
                        self.tracks
                            .capacity_at(true, root.tracks)?
                            .previous_growth_backing_bytes,
                    )
                    .and_then(|n| {
                        n.checked_add(
                            self.returns
                                .capacity_at(true, root.returns, root.feed)
                                .ok()?
                                .previous_growth_backing_bytes,
                        )
                    })
                    .and_then(|n| {
                        n.checked_add(
                            self.root
                                .capacity_at(true, project)
                                .ok()?
                                .previous_growth_backing_bytes,
                        )
                    })
                    .ok_or(AudioStorageError::Overflow)?
            } else {
                0
            },
        })
    }
    #[allow(clippy::too_many_arguments)] // One selected arm joins the existing constructor inputs.
    pub(super) fn reserve_with_effects(
        &mut self,
        slots: &[SourceSlot],
        inputs: &OwnedAudioInputs,
        project: &hex_project::Project,
        stages: Stages,
        samplers: Option<(
            &crate::native_device_resources::current_sampler::CurrentSamplerMetadata,
            &crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
        )>,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        if project
            .tracks()
            .iter()
            .any(hex_project::Track::retained_source_path)
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let Stages {
            assemble_tracks: tracks,
            assemble_returns: returns,
            assemble_preview: preview,
            assemble_project: root,
            ..
        } = stages;
        if self.phase != Phase::Vacant
            || !self.rows.is_empty()
            || self.staged
            || self.pending_audio.is_some()
            || self.pending_stream.is_some()
            || self.pending_metadata.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        // All identity/size/geometry checks precede construction of metadata.
        for s in slots {
            let source = inputs
                .sources
                .get(s.source)
                .ok_or(AudioStorageError::SourceMismatch)?;
            if s.owner.is_some()
                || s.prepared_origin.is_some()
                || s.disposal != Disposal::Unstarted
                || s.handle == 0
                || s.end <= s.start
                || s.source_length == 0
                || (!s.transformed
                    && s.source_offset
                        .checked_add(s.source_length)
                        .is_none_or(|n| {
                            u64::try_from(n).ok().is_none_or(|n| n > source.1.frames())
                        }))
                || (!s.looping && s.phase != 0)
                || (s.looping && s.phase >= s.source_length as u64)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            s.fade
                .validate_segment(s.end - s.start)
                .map_err(|_| AudioStorageError::SourceMismatch)?;
        }
        slots
            .len()
            .checked_mul(mem::size_of::<Pair>())
            .ok_or(AudioStorageError::Overflow)?;
        slots
            .len()
            .checked_mul(mem::size_of::<ClipMetadata>())
            .ok_or(AudioStorageError::Overflow)?;
        self.phase = Phase::Reserved;
        self.growth = if slots.len() > self.rows.capacity() {
            self.capacity(false)?.pair_backing_bytes
        } else {
            0
        };
        self.rows
            .try_reserve_exact(slots.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for (slot, s) in slots.iter().enumerate() {
            self.rows.push(Pair {
                transferred: false,
                binding: Binding {
                    slot,
                    track: s.track,
                    clip: s.clip,
                    source: s.source,
                    handle: s.handle,
                },
                clip: None,
                metadata: None,
                declaration: None,
                installed_handle: None,
                installed_slot: None,
                stream: None,
            });
            let index = self.rows.len() - 1;
            let target = &mut self.rows[index];
            let fade = (!s.fade.is_neutral()).then_some(s.fade);
            if s.gain != 1.0
                || fade.is_some()
                || s.phase != 0
                || matches!(&inputs.sources[s.source].1.kind, SourceKind::Streamed(_))
            {
                // Box allocation is process-fatal-OOM excluded, like existing
                // constructors. Install directly, with no later owning local.
                target.metadata = Some(Box::new(ClipMetadata {
                    gain: s.gain,
                    source_phase: s.phase,
                    fade,
                    streamed: None,
                }));
            }
        }
        if tracks {
            self.tracks.reserve_with_effects(
                project,
                &self.rows,
                samplers,
                effects,
                native_input,
            )?;
        }
        if returns {
            self.returns
                .reserve_with_effects(project, effects, native_input)?;
        }
        if preview {
            if samplers.is_some() {
                self.preview
                    .reserve_with_current_sampler(project, true, effects.is_some())?;
            } else {
                self.preview.reserve(project)?;
            }
        }
        if root {
            if samplers.is_some() {
                self.root.reserve_with_effects(project, true, effects)?;
            } else {
                self.root.reserve_with_effects(project, false, effects)?;
            }
        }
        #[cfg(test)]
        if self.mismatch
            && let Some(r) = self.rows.first_mut()
        {
            r.binding.handle = r
                .binding
                .handle
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(())
    }
    pub(super) fn compile_current_sampler_tracks(
        &mut self,
        project: &hex_project::Project,
        metadata: &crate::native_device_resources::current_sampler::CurrentSamplerMetadata,
        contexts: &crate::owned_shared::OwnedShared<
            crate::shared_analytic::SharedAnalyticContextTable,
        >,
    ) -> Result<(), AudioStorageError> {
        self.tracks
            .compile_current_sampler_tracks(project, metadata, contexts)
    }
    pub(super) fn main_quote(
        &self,
        project: &hex_project::Project,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(usize, usize), AudioStorageError> {
        self.root.main_quote(project, native_input)
    }
    pub(super) fn reserve_main_carrier(
        &mut self,
        project: &hex_project::Project,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        self.root
            .reserve_main_carrier(project, effects, native_input)
    }
    pub(super) fn construct(
        &mut self,
        slots: &mut [SourceSlot],
        inputs: &OwnedAudioInputs,
        tracks: bool,
        returns: bool,
        preview: bool,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Reserved || self.rows.len() != slots.len() {
            return Err(AudioStorageError::Capacity);
        }
        for (slot, (r, s)) in self.rows.iter().zip(slots.iter()).enumerate() {
            let source = s.source_owner(inputs)?;
            let owner = s.owner.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
            let required = s.gain != 1.0
                || matches!(&owner.kind, SourceKind::Streamed(_))
                || s.phase != 0
                || !s.fade.is_neutral();
            if r.binding
                != (Binding {
                    slot,
                    track: s.track,
                    clip: s.clip,
                    source: s.source,
                    handle: s.handle,
                })
                || r.transferred
                || r.clip.is_some()
                || r.installed_handle.is_some()
                || r.installed_slot.is_some()
                || r.declaration.is_some()
                || r.stream.is_some()
                || r.metadata.is_some() != required
                || r.metadata.as_ref().is_some_and(|m| {
                    m.streamed.is_some()
                        || m.gain != s.gain
                        || m.source_phase != s.phase
                        || m.fade != (!s.fade.is_neutral()).then_some(s.fade)
                })
                || s.disposal != Disposal::Unstarted
                || owner.handle != s.handle
                || !crate::audio_input_storage::same(owner, source)
                || crate::audio_input_storage::parts(owner)?
                    != crate::audio_input_storage::parts(source)?
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        // Batch preflight plus exclusive borrows make all following owner moves
        // infallible. No legacy consuming builder, allocation, clone or helper.
        for (index, (r, s)) in self.rows.iter_mut().zip(slots.iter_mut()).enumerate() {
            if let Some(OwnedAudioSource { kind, handle }) = s.owner.take() {
                match kind {
                    SourceKind::Resident(audio) => {
                        r.clip = Some(Clip {
                            active: s.active,
                            audio: Some(audio),
                            start: s.start,
                            end: s.end,
                            looping: s.looping,
                            source_offset: s.source_offset,
                            source_length: s.source_length,
                            metadata: r.metadata.take(),
                        });
                    }
                    SourceKind::Streamed(metadata) => {
                        let (arc, receipt) = metadata.into_paired_parts_on_control();
                        r.declaration = Some(receipt);
                        r.stream = Some(arc);
                        // Destination was checked above and cannot disappear.
                        if let Some(target) = r.metadata.as_mut() {
                            target.streamed = r.stream.take();
                        }
                        r.clip = Some(Clip {
                            active: s.active,
                            audio: None,
                            start: s.start,
                            end: s.end,
                            looping: s.looping,
                            source_offset: s.source_offset,
                            source_length: s.source_length,
                            metadata: r.metadata.take(),
                        });
                    }
                }
                r.installed_handle = Some(handle);
                r.installed_slot = Some(index);
                s.disposal = Disposal::Transferred;
            }
            #[cfg(test)]
            {
                assert_ne!(
                    self.panic_after,
                    Some(index),
                    "persistent Arrangement clip construction panic"
                );
                if self.fail_after == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
            }
        }
        self.phase = Phase::Installed;
        if tracks {
            self.tracks.construct(&mut self.rows)?;
        }
        if returns {
            self.returns.construct()?;
        }
        if preview {
            self.preview.construct()?;
        }
        Ok(())
    }
    pub(super) fn validate_root_sources(
        &self,
        slots: &[SourceSlot],
        bank: &OwnedAudioInputs,
    ) -> Result<(), AudioStorageError> {
        self.validate_root_sources_at(slots, bank, None)
    }
    pub(super) fn validate_root_sources_at(
        &self,
        slots: &[SourceSlot],
        bank: &OwnedAudioInputs,
        external: Option<&crate::Project>,
    ) -> Result<(), AudioStorageError> {
        self.tracks
            .validate_root_sources(self.root.view_at(external)?.tracks, slots, bank)
    }
    pub(crate) fn can_transfer_revision(&self) -> bool {
        self.root.project().is_some() && self.preview.can_transfer_revision()
    }
    pub(crate) fn can_restore_revision_partial(&self, project: bool, preview: bool) -> bool {
        (!project || self.root.can_restore_project())
            && (!preview || self.preview.can_restore_revision())
    }
    pub(crate) fn transfer_project_revision(&mut self, target: &mut Option<crate::Project>) {
        self.root.transfer_project(target);
    }
    pub(crate) fn transfer_preview_revision(
        &mut self,
        target: &mut Option<crate::MidiArrangementPreview>,
    ) {
        self.preview.transfer_revision(target);
    }
    pub(crate) fn restore_revision_values(
        &mut self,
        project: &mut Option<crate::Project>,
        preview: &mut Option<crate::MidiArrangementPreview>,
    ) {
        if project.is_some() {
            self.root.restore_project(project);
        }
        if preview.is_some() {
            self.preview.restore_revision(preview);
        }
    }
    pub(super) fn prepare_signal_graph(
        &mut self,
        canonical: &hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        self.root.prepare_signal_graph(canonical)
    }
    pub(super) fn construct_root(
        &mut self,
        shared: &mut crate::session_shared_preparation::SessionSharedConstruction,
    ) -> Result<(), AudioStorageError> {
        self.root
            .construct(&mut self.tracks, &mut self.returns, shared)
    }
    pub(super) fn restore_root(
        &mut self,
        shared: &mut crate::session_shared_preparation::SessionSharedConstruction,
    ) -> Result<(), AudioStorageError> {
        self.root
            .restore(&mut self.tracks, &mut self.returns, shared)
    }
    pub(super) fn root_context(
        &self,
    ) -> Option<&crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>>
    {
        self.root_context_at(None)
    }
    pub(super) fn root_context_at<'a>(
        &'a self,
        external: Option<&'a crate::Project>,
    ) -> Option<
        &'a crate::owned_shared::OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    > {
        self.root.context_at(external)
    }
    pub(super) fn has_next(&self) -> bool {
        !matches!(self.phase, Phase::Vacant | Phase::Complete)
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        self.root.dispose_signal_on_control()?;
        self.root.dispose_main_on_control()?;
        if self.preview.has_next() {
            self.preview.dispose_next()?;
            return Ok(());
        }
        if self.returns.has_next() {
            self.returns.dispose_next()?;
            return Ok(());
        }
        if self.tracks.has_next() {
            self.tracks.dispose_next(&mut self.rows)?;
            return Ok(());
        }
        self.phase = Phase::Cleaning;
        if self.cursor == self.rows.len() {
            self.phase = Phase::Complete;
            return Ok(());
        }
        if !self.staged {
            if self.pending_audio.is_some()
                || self.pending_stream.is_some()
                || self.pending_metadata.is_some()
            {
                return Err(AudioStorageError::Capacity);
            }
            // Cursor/steps persist BEFORE taking actual final owner. Empty
            // source-metadata/row destinations were validated above.
            self.staged = true;
            let r = &mut self.rows[self.cursor];
            if let Some(Clip {
                active: _,
                audio,
                metadata,
                start: _,
                end: _,
                looping: _,
                source_offset: _,
                source_length: _,
            }) = r.clip.take()
            {
                self.pending_audio = audio;
                self.pending_metadata = metadata;
            } else {
                self.pending_metadata = r.metadata.take();
            }
            if let Some(m) = self.pending_metadata.as_mut() {
                self.pending_stream = m.streamed.take();
            }
            if r.stream.is_some() {
                if self.pending_stream.is_some() {
                    return Err(AudioStorageError::UnknownStorage);
                }
                self.pending_stream = r.stream.take();
            }
            return Ok(());
        }
        if self.source_disposal == Disposal::Unstarted {
            self.source_disposal = Disposal::Pending;
            drop(self.pending_audio.take());
            drop(self.pending_stream.take());
            #[cfg(test)]
            drop(self.probe.take());
            self.source_disposal = Disposal::Complete;
            return Ok(());
        }
        if self.source_disposal != Disposal::Complete {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.metadata_disposal != Disposal::Unstarted {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.metadata_disposal = Disposal::Pending;
        drop(self.pending_metadata.take());
        self.metadata_disposal = Disposal::Complete;
        self.cursor += 1;
        self.staged = false;
        self.source_disposal = Disposal::Unstarted;
        self.metadata_disposal = Disposal::Unstarted;
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.staged
            || self.pending_audio.is_some()
            || self.pending_stream.is_some()
            || self.pending_metadata.is_some()
            || self.rows.iter().any(|r| {
                r.transferred || r.clip.is_some() || r.metadata.is_some() || r.stream.is_some()
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        self.preview.clear_disposed()?;
        self.returns.clear_disposed()?;
        self.tracks.clear_disposed()?;
        self.rows.clear();
        self.cursor = 0;
        self.phase = Phase::Vacant;
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn installed(&self) -> usize {
        self.installed_at(None)
    }
    pub(super) fn installed_at(&self, external: Option<&crate::Project>) -> usize {
        self.rows.iter().filter(|r| r.clip.is_some()).count()
            + self
                .tracks
                .installed_clips(self.root.view_at(external).ok().map_or(&[], |v| v.tracks))
    }
    #[cfg(test)]
    pub(super) fn return_metadata(&self) -> Option<returns::ArrangementReturnMetadata<'_>> {
        self.return_metadata_at(None)
    }
    #[cfg(test)]
    pub(super) fn track_metadata(
        &self,
        index: usize,
    ) -> Option<tracks::ArrangementTrackMetadata<'_>> {
        self.track_metadata_at(index, None)
    }
    pub(super) fn return_metadata_at<'a>(
        &'a self,
        external: Option<&'a crate::Project>,
    ) -> Option<returns::ArrangementReturnMetadata<'a>> {
        {
            let view = self.root.view_at(external).ok()?;
            self.returns.metadata_at(view.returns, view.feed)
        }
    }
    pub(super) fn preview_metadata_at(
        &self,
        external: Option<&crate::MidiArrangementPreview>,
    ) -> Option<crate::AudioPreviewMetadata> {
        self.preview.metadata_at(external)
    }
    pub(super) fn track_metadata_at<'a>(
        &'a self,
        index: usize,
        external: Option<&'a crate::Project>,
    ) -> Option<tracks::ArrangementTrackMetadata<'a>> {
        self.tracks
            .metadata(index, self.root.view_at(external).ok()?.tracks)
    }
    #[cfg(test)]
    pub(super) fn handles(&self) -> Vec<Option<u64>> {
        self.rows.iter().map(|r| r.installed_handle).collect()
    }
    #[cfg(test)]
    pub(super) fn pending_metadata_present(&self) -> bool {
        self.pending_metadata.is_some()
    }
    #[cfg(test)]
    pub(super) fn pending_stream_empty(&self) -> bool {
        self.pending_stream.is_none()
    }
    #[cfg(test)]
    pub(super) fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.rows.iter().filter_map(|r| r.clip.as_ref())
    }
    #[cfg(test)]
    pub(super) fn validate(&self, bank: &OwnedAudioInputs) -> bool {
        self.rows.iter().all(|r| {
            let Some(c) = &r.clip else {
                return false;
            };
            let Some((_, b)) = bank.sources.get(r.binding.source) else {
                return false;
            };
            r.installed_slot == Some(r.binding.slot)
                && r.installed_handle == Some(r.binding.handle)
                && match (&b.kind, &c.audio, &c.metadata) {
                    (SourceKind::Resident(a), Some(b), _) => {
                        a.shares_pcm_with(b) && r.declaration.is_none()
                    }
                    (SourceKind::Streamed(a), None, Some(m)) => {
                        m.streamed.as_ref().is_some_and(|b| {
                            Arc::ptr_eq(a.arc(), b)
                                && r.declaration.as_ref().and_then(|d| d.parts(b)) == a.parts()
                        })
                    }
                    _ => false,
                }
        })
    }
}

impl ArrangementClips {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        project: Option<&'a crate::Project>,
        external_preview: Option<&'a crate::MidiArrangementPreview>,
    ) -> Result<
        Option<
            &'a crate::owned_shared::OwnedShared<
                crate::shared_analytic::SharedAnalyticContextTable,
            >,
        >,
        AudioStorageError,
    > {
        self.capacity_at(false, project, external_preview)?;
        let Self {
            rows,
            tracks,
            root,
            returns,
            preview,
            phase: _,
            growth,
            cursor: _,
            pending_audio,
            pending_stream,
            pending_metadata,
            source_disposal: _,
            metadata_disposal: _,
            staged: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                mismatch: _,
            #[cfg(test)]
                probe: _,
        } = self;
        l.vec(rows)?;
        l.growth(*growth)?;
        let view = root.visit_audio_storage(l, project)?;
        tracks.visit_audio_storage(l, view.tracks)?;
        returns.visit_audio_storage(l, view.returns, view.feed)?;
        preview.visit_audio_storage(l, external_preview)?;
        for Pair {
            transferred: _,
            binding: _,
            clip,
            metadata,
            declaration,
            installed_handle,
            installed_slot: _,
            stream,
        } in rows
        {
            if let Some(clip) = clip {
                l.clip(
                    clip,
                    declaration.as_ref(),
                    installed_handle.ok_or(AudioStorageError::SourceMismatch)?,
                )?;
            }
            if let Some(metadata) = metadata {
                l.clip_metadata(metadata)?;
            }
            if stream.is_some() {
                l.unknown()?;
            }
        }
        if pending_audio.is_some() || pending_stream.is_some() {
            l.unknown()?;
        }
        if let Some(metadata) = pending_metadata {
            l.clip_metadata(metadata)?;
        }
        Ok(root.context_at(project))
    }
}
