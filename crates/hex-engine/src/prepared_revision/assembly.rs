//! Private four-allocation revision composition from existing sealed children.
//! This establishes constructor declarations and retained custody, never complete
//! accounting, publication, callback ownership or lifetime-closure admission.
use super::*;
use crate::{
    AudioStorageError as Error, audio_preparation::ArrangementClips, owned_shared::OwnedShared,
    session_inventory::audio_assembly::SessionAssembly,
};

/// Requested-layout declarations for the four private shared root allocations.
/// Child heap backings remain charged by their existing stage reports. Reserved
/// bytes conservatively remain charged through incomplete control extraction;
/// these fields never establish whole-graph storage or publication eligibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioRevisionRootCapacity {
    pub reserved_handle_destinations: usize,
    pub reserved_data_bytes: usize,
    pub actual_allocations: usize,
    pub actual_payload_bytes: usize,
    pub actual_declared_overhead_bytes: usize,
    pub control_extracted_allocations: usize,
    pub constructed: bool,
}
impl AudioRevisionRootCapacity {
    pub(crate) fn bytes(self) -> usize {
        self.reserved_data_bytes
    }
}
/// Restricted scalar metadata. No raw Project/Session/preview/Arc/Weak owner.
/// ```compile_fail
/// fn escape(v: hex_engine::AudioPreparedRevisionMetadata) { let _ = v.arrangement(); }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPreparedRevisionMetadata {
    pub sample_rate: u32,
    pub track_count: usize,
    pub return_count: usize,
    pub contains_streamed_sources: bool,
}
#[derive(Default)]
pub(crate) struct RevisionAssembly {
    owner: Option<OwnedShared<PreparedRevision>>,
    revision_value: Option<PreparedRevision>,
    arrangement: Option<OwnedShared<PlaybackProject>>,
    session: Option<OwnedShared<PreparedSessionInventory>>,
    preview: Option<OwnedShared<MidiArrangementPreview>>,
    arrangement_value: Option<PlaybackProject>,
    session_value: Option<PreparedSessionInventory>,
    preview_value: Option<MidiArrangementPreview>,
    mutable_descriptors: Vec<PreparedMutableTrackRevision>,
    handles: [u64; 4],
    reserved_bytes: usize,
    reserved: bool,
    current_sampler_scope: Option<u64>,
    native_parameter_input: Option<crate::device_automation::NativeParameterInputTarget>,
    extraction_attempted: [bool; 4],
    extracted: usize,
    #[cfg(test)]
    pub(crate) fail_after: Option<usize>,
    #[cfg(test)]
    pub(crate) panic_after: Option<usize>,
}
pub(crate) struct RevisionStorageView<'a> {
    pub(crate) arrangement: Option<&'a PlaybackProject>,
    pub(crate) session: Option<&'a PreparedSessionInventory>,
    pub(crate) preview: Option<&'a MidiArrangementPreview>,
}
fn add_parts<T>(
    owner: &OwnedShared<T>,
    report: &mut AudioRevisionRootCapacity,
) -> Result<(), Error> {
    let (payload, overhead) = owner.parts().ok_or(Error::UnsupportedConstructor)?;
    report.actual_payload_bytes = report
        .actual_payload_bytes
        .checked_add(payload)
        .ok_or(Error::Overflow)?;
    report.actual_declared_overhead_bytes = report
        .actual_declared_overhead_bytes
        .checked_add(overhead)
        .ok_or(Error::Overflow)?;
    report.actual_allocations = report
        .actual_allocations
        .checked_add(1)
        .ok_or(Error::Overflow)?;
    Ok(())
}
impl RevisionAssembly {
    pub(crate) fn candidate_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.owner.iter().flat_map(|owner| {
            owner
                .arrangement
                .tracks
                .iter()
                .flat_map(|track| track.clips.iter())
                .filter_map(|clip| {
                    clip.metadata
                        .as_ref()
                        .and_then(|metadata| metadata.streamed.as_deref())
                })
                .chain(
                    owner
                        .session
                        .audio_clips()
                        .iter()
                        .filter_map(|clip| match clip.source() {
                            crate::compile::PreparedAudioSource::Resident(_) => None,
                            crate::compile::PreparedAudioSource::Streamed(metadata) => {
                                Some(metadata.as_ref())
                            }
                        }),
                )
        })
    }
    /// Constructor-only control sharing. The returned handle is immediately
    /// secured inside the opaque candidate, never exposed to an application.
    pub(crate) fn candidate_root(&self) -> Result<std::sync::Arc<PreparedRevision>, Error> {
        Ok(std::sync::Arc::clone(self.candidate_root_view()?))
    }
    pub(crate) fn candidate_root_view(&self) -> Result<&std::sync::Arc<PreparedRevision>, Error> {
        self.capacity()?;
        let owner = self.owner.as_ref().ok_or(Error::UnsupportedGraph)?;
        if owner.parts().is_none() {
            return Err(Error::UnknownStorage);
        }
        Ok(owner.arc())
    }

    pub(crate) fn reserve_with_sampler_and_effects(
        &mut self,
        project: &hex_project::Project,
        scope: Option<u64>,
        effects: bool,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), Error> {
        if self.reserved
            || self.owner.is_some()
            || self.revision_value.is_some()
            || self.arrangement.is_some()
            || self.session.is_some()
            || self.preview.is_some()
            || self.arrangement_value.is_some()
            || self.session_value.is_some()
            || self.preview_value.is_some()
            || self.mutable_descriptors.capacity() != 0
            || self.handles != [0; 4]
        {
            return Err(Error::Capacity);
        }
        if scope.is_some() {
            if !crate::native_device_resources::current_sampler::validate_project_with_effects(
                project, effects,
            )? {
                return Err(Error::UnsupportedGraph);
            }
        } else {
            // Empty mutable descriptors are permitted only for this exhaustive class.
            for track in project.tracks() {
                if track.instrument().is_some()
                    || track.devices().iter().any(|d| {
                        !effects && d.state().type_id() != hex_devices::utility::UTILITY_TYPE_ID
                    })
                    || !track.midi_clips().is_empty()
                {
                    return Err(Error::UnsupportedGraph);
                }
            }
            for bus in project.return_tracks() {
                if !effects && !bus.devices().is_empty() {
                    return Err(Error::UnsupportedGraph);
                }
            }
            for cell in project.session_cells() {
                match cell.content() {
                    hex_project::SessionCellContent::Audio(_)
                    | hex_project::SessionCellContent::Stop => {}
                    hex_project::SessionCellContent::Midi(_) => {
                        return Err(Error::UnsupportedGraph);
                    }
                }
            }
        }
        self.current_sampler_scope = scope;
        self.native_parameter_input = native_input;
        self.reserved_bytes = 0;
        for parts in [
            OwnedShared::<PlaybackProject>::declared_parts(),
            OwnedShared::<PreparedSessionInventory>::declared_parts(),
            OwnedShared::<MidiArrangementPreview>::declared_parts(),
            OwnedShared::<PreparedRevision>::declared_parts(),
        ] {
            let (payload, overhead) = parts.ok_or(Error::UnsupportedConstructor)?;
            self.reserved_bytes = self
                .reserved_bytes
                .checked_add(payload)
                .and_then(|n| n.checked_add(overhead))
                .ok_or(Error::Overflow)?;
        }
        self.reserved_bytes = self
            .reserved_bytes
            .checked_add(crate::PreparedSourceRouting::construction_bytes(project))
            .ok_or(Error::Overflow)?;
        self.reserved = true;
        for index in 0..4 {
            self.handles[index] = crate::audio_only_inputs::handle()?;
            #[cfg(test)]
            self.fault(index)?;
        }
        Ok(())
    }
    pub(crate) fn storage_view(&self) -> Result<RevisionStorageView<'_>, Error> {
        if self.owner.is_some() && self.revision_value.is_some() {
            return Err(Error::SourceMismatch);
        }
        let revision = self
            .owner
            .as_ref()
            .map(|o| o.as_ref())
            .or(self.revision_value.as_ref());
        if let Some(PreparedRevision {
            source_routing: _,
            resource_scope,
            device_resource_scope,
            tuning_family: _,
            arrangement,
            session,
            midi_preview,
            mutable_track_revisions,
            authored_midi_sources,
            gain_recording_track,
            native_parameter_input,
            scalar_automation,
        }) = revision
        {
            if *resource_scope != self.current_sampler_scope
                || device_resource_scope.is_some()
                || scalar_automation.is_some()
            {
                return Err(Error::UnsupportedGraph);
            }
            if self.arrangement.is_some()
                || self.session.is_some()
                || self.preview.is_some()
                || self.arrangement_value.is_some()
                || self.session_value.is_some()
                || self.preview_value.is_some()
                || gain_recording_track.is_some()
                || *native_parameter_input != self.native_parameter_input
                || mutable_track_revisions.capacity() != 0
                || authored_midi_sources.capacity() != 0
                || self.mutable_descriptors.capacity() != 0
            {
                return Err(Error::UnknownStorage);
            }
            return Ok(RevisionStorageView {
                arrangement: Some(arrangement.as_ref()),
                session: Some(session.as_ref()),
                preview: Some(midi_preview.as_ref()),
            });
        }
        if (self.arrangement.is_some() && self.arrangement_value.is_some())
            || (self.session.is_some() && self.session_value.is_some())
            || (self.preview.is_some() && self.preview_value.is_some())
            || self.mutable_descriptors.capacity() != 0
        {
            return Err(Error::UnknownStorage);
        }
        Ok(RevisionStorageView {
            arrangement: self
                .arrangement
                .as_ref()
                .map(|o| o.as_ref())
                .or(self.arrangement_value.as_ref()),
            session: self
                .session
                .as_ref()
                .map(|o| o.as_ref())
                .or(self.session_value.as_ref()),
            preview: self
                .preview
                .as_ref()
                .map(|o| o.as_ref())
                .or(self.preview_value.as_ref()),
        })
    }
    pub(crate) fn capacity(&self) -> Result<AudioRevisionRootCapacity, Error> {
        self.storage_view()?;
        let mut report = AudioRevisionRootCapacity {
            reserved_handle_destinations: self.handles.iter().filter(|h| **h != 0).count(),
            reserved_data_bytes: self.reserved_bytes,
            control_extracted_allocations: self.extracted,
            constructed: self.owner.is_some(),
            ..Default::default()
        };
        if let Some(owner) = &self.owner {
            add_parts(owner, &mut report)?;
        }
        let revision = self
            .owner
            .as_ref()
            .map(|o| o.as_ref())
            .or(self.revision_value.as_ref());
        if let Some(revision) = revision {
            add_parts(&revision.arrangement, &mut report)?;
            add_parts(&revision.session, &mut report)?;
            add_parts(&revision.midi_preview, &mut report)?;
        } else {
            if let Some(owner) = &self.arrangement {
                add_parts(owner, &mut report)?;
            }
            if let Some(owner) = &self.session {
                add_parts(owner, &mut report)?;
            }
            if let Some(owner) = &self.preview {
                add_parts(owner, &mut report)?;
            }
        }
        if report.actual_allocations > 0
            && (self.handles.contains(&0)
                || self
                    .handles
                    .iter()
                    .enumerate()
                    .any(|(i, h)| self.handles[..i].contains(h)))
        {
            return Err(Error::SourceMismatch);
        }
        if report
            .actual_payload_bytes
            .checked_add(report.actual_declared_overhead_bytes)
            .ok_or(Error::Overflow)?
            > report.reserved_data_bytes
        {
            return Err(Error::ReservationExceeded);
        }
        Ok(report)
    }
    pub(crate) fn construct(
        &mut self,
        arrangement: &mut ArrangementClips,
        session: &mut SessionAssembly,
        source_routing: crate::PreparedSourceRouting,
    ) -> Result<(), Error> {
        if !self.reserved
            || self.handles.contains(&0)
            || !arrangement.can_transfer_revision()
            || !session.can_transfer_revision()
            || self.capacity()?.actual_allocations != 0
            || self.arrangement_value.is_some()
            || self.session_value.is_some()
            || self.preview_value.is_some()
        {
            return Err(Error::SourceMismatch);
        }
        arrangement.transfer_project_revision(&mut self.arrangement_value);
        #[cfg(test)]
        self.fault(4)?;
        if let Some(value) = self.arrangement_value.take() {
            self.arrangement = Some(OwnedShared::new_owned(value));
        }
        #[cfg(test)]
        self.fault(5)?;
        session.transfer_revision(&mut self.session_value);
        #[cfg(test)]
        self.fault(6)?;
        if let Some(value) = self.session_value.take() {
            self.session = Some(OwnedShared::new_owned(value));
        }
        #[cfg(test)]
        self.fault(7)?;
        arrangement.transfer_preview_revision(&mut self.preview_value);
        #[cfg(test)]
        self.fault(8)?;
        if let Some(value) = self.preview_value.take() {
            self.preview = Some(OwnedShared::new_owned(value));
        }
        #[cfg(test)]
        self.fault(9)?;
        self.capacity()?;
        // Take and secure each child in one persistent revision value. All
        // destinations were preflighted; no callback, reserve or probe intervenes.
        if self.arrangement.is_none() || self.session.is_none() || self.preview.is_none() {
            return Err(Error::UnknownStorage);
        }
        match (
            self.arrangement.take(),
            self.session.take(),
            self.preview.take(),
        ) {
            (Some(arrangement), Some(session), Some(midi_preview)) => {
                self.revision_value = Some(PreparedRevision {
                    source_routing,
                    resource_scope: self.current_sampler_scope,
                    device_resource_scope: None,
                    tuning_family: TuningContinuationFamily::fresh()
                        .map_err(|_| Error::Overflow)?,
                    arrangement,
                    session,
                    midi_preview,
                    mutable_track_revisions: Vec::new(),
                    authored_midi_sources: Vec::new(),
                    gain_recording_track: None,
                    native_parameter_input: self.native_parameter_input,
                    scalar_automation: None,
                });
            }
            (arrangement, session, preview) => {
                self.arrangement = arrangement;
                self.session = session;
                self.preview = preview;
                return Err(Error::SourceMismatch);
            }
        }
        #[cfg(test)]
        self.fault(10)?;
        if let Some(value) = self.revision_value.take() {
            self.owner = Some(OwnedShared::new_owned(value));
        }
        #[cfg(test)]
        self.fault(11)?;
        self.capacity()?;
        Ok(())
    }
    pub(crate) fn restore(
        &mut self,
        arrangement: &mut ArrangementClips,
        session: &mut SessionAssembly,
    ) -> Result<(), Error> {
        if !self.reserved {
            return Ok(());
        }
        self.capacity()?;
        // Only destinations for children already moved here must be vacant.
        let view = self.storage_view()?;
        if (view.arrangement.is_some() || view.preview.is_some())
            && !arrangement
                .can_restore_revision_partial(view.arrangement.is_some(), view.preview.is_some())
        {
            return Err(Error::Capacity);
        }
        if view.session.is_some() && !session.can_restore_revision() {
            return Err(Error::Capacity);
        }
        arrangement.capacity_at(false, view.arrangement, view.preview)?;
        session.capacity_at(false, view.session)?;
        if self.extraction_attempted.iter().any(|attempted| *attempted) {
            return Err(Error::UnknownStorage);
        }
        if self.owner.is_some() {
            self.extraction_attempted[3] = true;
            OwnedShared::restore_value_on_control(&mut self.owner, &mut self.revision_value)?;
            self.extracted += 1;
        }
        if let Some(PreparedRevision {
            source_routing: _,
            resource_scope: _,
            device_resource_scope: _,
            tuning_family: _,
            arrangement,
            session,
            midi_preview,
            mutable_track_revisions,
            authored_midi_sources: _,
            gain_recording_track: _,
            native_parameter_input: _,
            scalar_automation: _,
        }) = self.revision_value.take()
        {
            self.arrangement = Some(arrangement);
            self.session = Some(session);
            self.preview = Some(midi_preview);
            self.mutable_descriptors = mutable_track_revisions;
        }
        if self.arrangement.is_some() {
            self.extraction_attempted[0] = true;
            OwnedShared::restore_value_on_control(
                &mut self.arrangement,
                &mut self.arrangement_value,
            )?;
            self.extracted += 1;
        }
        if self.session.is_some() {
            self.extraction_attempted[1] = true;
            OwnedShared::restore_value_on_control(&mut self.session, &mut self.session_value)?;
            self.extracted += 1;
        }
        if self.preview.is_some() {
            self.extraction_attempted[2] = true;
            OwnedShared::restore_value_on_control(&mut self.preview, &mut self.preview_value)?;
            self.extracted += 1;
        }
        arrangement.restore_revision_values(&mut self.arrangement_value, &mut self.preview_value);
        if self.session_value.is_some() {
            session.restore_revision(&mut self.session_value);
        }
        self.current_sampler_scope = None;
        self.native_parameter_input = None;
        self.reserved = false;
        self.handles = [0; 4];
        self.reserved_bytes = 0;
        self.extraction_attempted = [false; 4];
        self.extracted = 0;
        Ok(())
    }
    pub(crate) fn metadata(&self) -> Option<AudioPreparedRevisionMetadata> {
        let revision = self.owner.as_ref()?;
        Some(AudioPreparedRevisionMetadata {
            sample_rate: revision.arrangement.sample_rate(),
            track_count: revision.arrangement.tracks.len(),
            return_count: revision.arrangement.return_buses.len(),
            contains_streamed_sources: revision.contains_streamed_sources(),
        })
    }
    #[cfg(test)]
    pub(crate) fn owner_for_test(&self) -> Option<&OwnedShared<PreparedRevision>> {
        self.owner.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn corrupt_handle_for_test(&mut self) {
        self.handles[1] = self.handles[0];
    }
    #[cfg(test)]
    fn fault(&self, frontier: usize) -> Result<(), Error> {
        assert_ne!(
            self.panic_after,
            Some(frontier),
            "persistent private revision root panic"
        );
        if self.fail_after == Some(frontier) {
            return Err(Error::Allocation);
        }
        Ok(())
    }
}

impl RevisionAssembly {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<RevisionStorageView<'a>, Error> {
        use crate::audio_storage::Owner;
        self.capacity()?;
        let Self {
            owner,
            revision_value,
            arrangement,
            session,
            preview,
            arrangement_value: _,
            session_value: _,
            preview_value: _,
            mutable_descriptors,
            handles,
            reserved_bytes: _,
            reserved: _,
            current_sampler_scope: _,
            extraction_attempted: _,
            extracted: _,
            native_parameter_input: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        l.vec(mutable_descriptors)?;
        if let Some(owner) = owner {
            l.shared(
                owner.arc(),
                owner.parts(),
                Owner::exclusive(owner),
                handles[3],
                None,
            )?;
        }
        if let Some(revision) = owner
            .as_ref()
            .map(|r| r.as_ref())
            .or(revision_value.as_ref())
        {
            let PreparedRevision {
                source_routing,
                resource_scope: _,
                device_resource_scope: _,
                tuning_family: _,
                arrangement,
                session,
                midi_preview,
                mutable_track_revisions,
                authored_midi_sources,
                gain_recording_track: _,
                native_parameter_input: _,
                scalar_automation: _,
            } = revision;
            source_routing.visit_storage(l)?;
            l.vec(mutable_track_revisions)?;
            // This constructor class never admits ordinary authored-evidence children.
            if authored_midi_sources.capacity() != 0 {
                return Err(Error::UnknownStorage);
            }
            l.shared(
                arrangement.arc(),
                arrangement.parts(),
                Owner::exclusive(arrangement),
                handles[0],
                None,
            )?;
            l.shared(
                session.arc(),
                session.parts(),
                Owner::exclusive(session),
                handles[1],
                None,
            )?;
            l.shared(
                midi_preview.arc(),
                midi_preview.parts(),
                Owner::exclusive(midi_preview),
                handles[2],
                None,
            )?;
        } else {
            if let Some(owner) = arrangement {
                l.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(owner),
                    handles[0],
                    None,
                )?;
            }
            if let Some(owner) = session {
                l.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(owner),
                    handles[1],
                    None,
                )?;
            }
            if let Some(owner) = preview {
                l.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(owner),
                    handles[2],
                    None,
                )?;
            }
        }
        self.storage_view()
    }
}
