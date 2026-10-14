//! Exact control-side products for one accepted canonical project revision.
use crate::{
    Invalid, MidiArrangementPreview, PreparedSessionInventory, Project as PlaybackProject,
    compile::PreparedAudioSource,
};
use hex_devices::SineTable;
use hex_project::{
    AssetId, Device, DeviceId, MAX_CLIPS, MAX_DEVICES_PER_TRACK, MAX_TRACKS, MidiSourceIdentity,
    Project, TrackId,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, PartialEq, Eq)]
struct PreparedDeviceIdentity {
    id: DeviceId,
    state: hex_lattice_contract::NativeDeviceState,
    bypassed: bool,
}

impl From<&Device> for PreparedDeviceIdentity {
    fn from(device: &Device) -> Self {
        Self {
            id: device.id(),
            state: device.state().clone().without_output_gain(),
            bypassed: device.bypassed(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedArrangementMidiSource {
    id: hex_project::ClipId,
    start: hex_project::BeatTime,
    length: hex_project::BeatTime,
    source: MidiSourceIdentity,
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedMutableTrackRevision {
    id: TrackId,
    instrument: Option<PreparedDeviceIdentity>,
    instrument_position: Option<usize>,
    effects: Vec<PreparedDeviceIdentity>,
    arrangement_midi_sources: Vec<PreparedArrangementMidiSource>,
}

impl PreparedMutableTrackRevision {
    fn topology_matches(&self, other: &Self) -> bool {
        self.id == other.id
            && self.instrument == other.instrument
            && self.instrument_position == other.instrument_position
            && self.effects == other.effects
    }

    /// Retargeting may change authored instrument state, but it may not alter
    /// the instrument identity, bypass/slot, position, effects, or track.
    fn retarget_topology_matches(&self, other: &Self) -> bool {
        self.id == other.id
            && self
                .instrument
                .as_ref()
                .zip(other.instrument.as_ref())
                .is_some_and(|(old, new)| old.id == new.id && old.bypassed == new.bypassed)
            && self.instrument_position == other.instrument_position
            && self.effects == other.effects
    }

    fn arrangement_sources_match(&self, other: &Self) -> bool {
        self.arrangement_midi_sources == other.arrangement_midi_sources
    }
}

/// Immutable compiled outputs prepared before a canonical revision is committed.
/// Arrangement playback and Session inventory share a stable instrument table;
/// runtime route ownership is maintained by the playback kernel.
pub struct PreparedRevision {
    arrangement: Arc<PlaybackProject>,
    session: Arc<PreparedSessionInventory>,
    midi_preview: Arc<MidiArrangementPreview>,
    mutable_track_revisions: Vec<PreparedMutableTrackRevision>,
}

impl std::fmt::Debug for PreparedRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRevision")
            .field("arrangement_rate", &self.arrangement.sample_rate())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl PreparedRevision {
    pub fn prepare(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
    ) -> Result<Self, Invalid> {
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_instrument_registry(
            project,
            media,
            sine_table,
            previous_session,
            &instruments,
        )
    }

    pub(crate) fn prepare_with_instrument_registry(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, Invalid> {
        crate::gain_envelope::validate_project(project)?;
        crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        let shared_contexts = crate::shared_analytic::SharedAnalyticContextTable::prepare(
            project,
            instruments,
            previous_session.map(|previous| previous.shared_contexts().as_ref()),
        )?;
        if shared_contexts.payload_bytes()
            > crate::shared_analytic::MAX_SHARED_CONTEXT_BYTES_PER_REVISION
        {
            return Err(Invalid(
                "shared instrument context revision budget exceeded",
            ));
        }
        let (arrangement, midi_preview) =
            crate::compile::project_with_midi_preview_and_table_and_instrument_contexts(
                project,
                media,
                Some(Arc::clone(&sine_table)),
                &effects,
                instruments,
                Some(Arc::clone(&shared_contexts)),
            )?;
        let session = PreparedSessionInventory::prepare_with_media_and_shared_contexts(
            project,
            media,
            sine_table,
            previous_session,
            instruments,
            shared_contexts,
        )?;
        crate::mutable_instrument::validate_prepared_graph_budget(&arrangement, &session)
            .map_err(|_| Invalid("active effect and mutable instrument runtime budget exceeded"))?;
        let mutable_track_revisions = prepare_mutable_track_revisions(project, &arrangement)?;
        let midi_bank_bytes = arrangement
            .midi_note_bank_storage_bytes()?
            .checked_add(session.midi_note_bank_storage_bytes())
            .ok_or(Invalid("complete revision midi bank byte count overflow"))?;
        let midi_bank_references = arrangement
            .midi_note_sampler_reference_count()?
            .checked_add(session.midi_note_bank_reference_count())
            .ok_or(Invalid(
                "complete revision midi bank reference count overflow",
            ))?;
        crate::midi_note_bank::PreparedMidiNoteBank::validate_complete_revision_budget(
            midi_bank_bytes,
            midi_bank_references,
        )?;
        Ok(Self {
            arrangement: Arc::new(arrangement),
            session: Arc::new(session),
            midi_preview: Arc::new(midi_preview),
            mutable_track_revisions,
        })
    }

    /// Hidden fixture seam for integration tests that need the real prepared
    /// revision pipeline with an injected mutable instrument definition.
    /// This method is absent from ordinary engine builds.
    #[cfg(feature = "instrument-test-support")]
    #[doc(hidden)]
    pub fn prepare_with_test_instrument_registry(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, Invalid> {
        Self::prepare_with_instrument_registry(
            project,
            media,
            sine_table,
            previous_session,
            instruments,
        )
    }

    /// Prepare Arrangement playback and the Session inventory from one mixed
    /// resident/streamed bank. Streamed bindings are validated against the
    /// supplied nonzero project-open generation by the shared compiler resolver.
    pub fn prepare_with_prepared_audio_sources(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
    ) -> Result<Self, Invalid> {
        crate::gain_envelope::validate_project(project)?;
        crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        let shared_contexts = crate::shared_analytic::SharedAnalyticContextTable::prepare(
            project,
            &instruments,
            previous_session.map(|previous| previous.shared_contexts().as_ref()),
        )?;
        if shared_contexts.payload_bytes()
            > crate::shared_analytic::MAX_SHARED_CONTEXT_BYTES_PER_REVISION
        {
            return Err(Invalid(
                "shared instrument context revision budget exceeded",
            ));
        }
        let (arrangement, midi_preview) =
            crate::compile::project_with_prepared_audio_sources_and_midi_preview_and_contexts(
                project,
                sources,
                expected_generation,
                Some(Arc::clone(&sine_table)),
                Some(Arc::clone(&shared_contexts)),
            )?;
        let session =
            PreparedSessionInventory::prepare_with_prepared_audio_sources_and_shared_contexts(
                project,
                sources,
                expected_generation,
                sine_table,
                previous_session,
                &instruments,
                shared_contexts,
            )?;
        crate::mutable_instrument::validate_prepared_graph_budget(&arrangement, &session)
            .map_err(|_| Invalid("active effect and mutable instrument runtime budget exceeded"))?;
        let mutable_track_revisions = prepare_mutable_track_revisions(project, &arrangement)?;
        let midi_bank_bytes = arrangement
            .midi_note_bank_storage_bytes()?
            .checked_add(session.midi_note_bank_storage_bytes())
            .ok_or(Invalid("complete revision midi bank byte count overflow"))?;
        let midi_bank_references = arrangement
            .midi_note_sampler_reference_count()?
            .checked_add(session.midi_note_bank_reference_count())
            .ok_or(Invalid(
                "complete revision midi bank reference count overflow",
            ))?;
        crate::midi_note_bank::PreparedMidiNoteBank::validate_complete_revision_budget(
            midi_bank_bytes,
            midi_bank_references,
        )?;
        Ok(Self {
            arrangement: Arc::new(arrangement),
            session: Arc::new(session),
            midi_preview: Arc::new(midi_preview),
            mutable_track_revisions,
        })
    }

    pub fn arrangement(&self) -> &Arc<PlaybackProject> {
        &self.arrangement
    }

    pub fn session(&self) -> &Arc<PreparedSessionInventory> {
        &self.session
    }

    pub fn midi_preview(&self) -> &Arc<MidiArrangementPreview> {
        &self.midi_preview
    }

    pub fn contains_streamed_sources(&self) -> bool {
        self.arrangement.contains_streamed_sources() || self.session.contains_streamed_sources()
    }

    /// Borrow every streamed binding without allocating or cloning an owner.
    /// Arrangement bindings follow compiled track/clip order, then Session
    /// bindings follow `audio_clips()` inventory order. Repeated references to
    /// one source are yielded separately; resident audio and MIDI are omitted.
    /// Metadata remains immutable and borrowed from this exact revision.
    pub fn streamed_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> + '_ {
        self.arrangement
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter_map(|clip| clip.metadata.as_ref()?.streamed.as_deref())
            .chain(
                self.session
                    .audio_clips()
                    .iter()
                    .filter_map(|clip| match clip.source() {
                        PreparedAudioSource::Streamed(source) => Some(source.as_ref()),
                        PreparedAudioSource::Resident(_) => None,
                    }),
            )
    }

    /// Whether the compiled Arrangement owns any enabled mutable instrument.
    /// Session-only inventory must not affect linear Arrangement range policy.
    pub fn has_arrangement_mutable_instruments(&self) -> bool {
        self.arrangement
            .tracks()
            .iter()
            .any(crate::Track::has_mutable_instrument)
    }

    /// Whether an Arrangement seek must reconstruct stateful history instead
    /// of using the direct absolute-frame sampler path. Built-in Sine and
    /// Operator MIDI notes are deterministic functions of note age and gate
    /// duration, so they can be evaluated directly at a seek destination.
    /// Mutable Arrangement instruments and stateful track/return processors
    /// carry history and require preroll. Return processors are checked
    /// conservatively even when no current send reaches them.
    pub fn arrangement_seek_requires_preroll(&self) -> bool {
        self.arrangement
            .tracks()
            .iter()
            .any(|track| track.has_mutable_instrument() || track.has_device_chain())
            || self.arrangement.return_buses().iter().any(|bus| {
                bus.device_chain
                    .iter()
                    .any(|device| device.definition.is_some())
            })
    }

    pub(crate) fn mutable_track_topology_matches(&self, other: &Self, track: TrackId) -> bool {
        let Some(old) = self.mutable_track_revision(track) else {
            return false;
        };
        let Some(new) = other.mutable_track_revision(track) else {
            return false;
        };
        old.topology_matches(new)
    }

    pub(crate) fn mutable_track_retarget_topology_matches(
        &self,
        other: &Self,
        track: TrackId,
    ) -> bool {
        let Some(old) = self.mutable_track_revision(track) else {
            return false;
        };
        let Some(new) = other.mutable_track_revision(track) else {
            return false;
        };
        old.retarget_topology_matches(new)
    }

    pub(crate) fn mutable_arrangement_track_matches(&self, other: &Self, track: TrackId) -> bool {
        let Some(old) = self.mutable_track_revision(track) else {
            return false;
        };
        let Some(new) = other.mutable_track_revision(track) else {
            return false;
        };
        old.topology_matches(new) && old.arrangement_sources_match(new)
    }

    pub(crate) fn mutable_arrangement_retarget_matches(
        &self,
        other: &Self,
        track: TrackId,
    ) -> bool {
        let Some(old) = self.mutable_track_revision(track) else {
            return false;
        };
        let Some(new) = other.mutable_track_revision(track) else {
            return false;
        };
        old.retarget_topology_matches(new) && old.arrangement_sources_match(new)
    }

    fn mutable_track_revision(&self, track: TrackId) -> Option<&PreparedMutableTrackRevision> {
        self.mutable_track_revisions
            .binary_search_by_key(&track, |revision| revision.id)
            .ok()
            .and_then(|index| self.mutable_track_revisions.get(index))
    }
}

fn prepare_mutable_track_revisions(
    project: &Project,
    arrangement: &PlaybackProject,
) -> Result<Vec<PreparedMutableTrackRevision>, Invalid> {
    if project.tracks().len() > MAX_TRACKS || arrangement.tracks().len() > MAX_TRACKS {
        return Err(Invalid(
            "mutable revision track count exceeds project limit",
        ));
    }
    if arrangement.tracks().len() != project.tracks().len() {
        return Err(Invalid(
            "mutable revision descriptors do not match compiled tracks",
        ));
    }
    let mutable_track_count = arrangement
        .tracks()
        .iter()
        .filter(|track| track.has_mutable_instrument())
        .count();
    let mut revisions = Vec::new();
    revisions
        .try_reserve_exact(mutable_track_count)
        .map_err(|_| Invalid("could not reserve mutable revision descriptors"))?;
    let mut arrangement_clip_count = 0usize;
    for (track, compiled) in project.tracks().iter().zip(arrangement.tracks()) {
        arrangement_clip_count = arrangement_clip_count
            .checked_add(track.midi_clips().len())
            .filter(|count| *count <= MAX_CLIPS)
            .ok_or(Invalid(
                "mutable revision MIDI clip count exceeds project limit",
            ))?;
        if compiled.canonical_track_id() != Some(track.id()) {
            return Err(Invalid(
                "mutable revision descriptor track ordering is inconsistent",
            ));
        }
        if !compiled.has_mutable_instrument() {
            continue;
        }
        if track.devices().len() > MAX_DEVICES_PER_TRACK {
            return Err(Invalid("mutable revision effect count exceeds track limit"));
        }
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(track.devices().len())
            .map_err(|_| Invalid("could not reserve mutable effect descriptors"))?;
        effects.extend(track.devices().iter().map(PreparedDeviceIdentity::from));

        let mut arrangement_midi_sources = Vec::new();
        arrangement_midi_sources
            .try_reserve_exact(track.midi_clips().len())
            .map_err(|_| Invalid("could not reserve mutable midi source descriptors"))?;
        arrangement_midi_sources.extend(track.midi_clips().iter().map(|clip| {
            PreparedArrangementMidiSource {
                id: clip.id(),
                start: clip.start(),
                length: clip.length(),
                source: clip.source_identity(),
            }
        }));
        arrangement_midi_sources.sort_unstable_by_key(|clip| clip.id);

        revisions.push(PreparedMutableTrackRevision {
            id: track.id(),
            instrument: track.instrument().map(PreparedDeviceIdentity::from),
            instrument_position: track.instrument_position(),
            effects,
            arrangement_midi_sources,
        });
    }
    revisions.sort_unstable_by_key(|revision| revision.id);
    Ok(revisions)
}

impl PreparedRevision {
    pub(crate) fn visit_owned_storage<'a, V: crate::storage_report::PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut crate::storage_report::Report<'a, '_, V>,
        owner: crate::storage_report::StorageOwner<'a>,
    ) -> Result<(), crate::storage_report::StorageReportError> {
        let Self {
            arrangement: _,
            session: _,
            midi_preview: _,
            mutable_track_revisions,
        } = self;
        report.vec(
            owner,
            "revision.mutable_tracks",
            [0; 2],
            mutable_track_revisions,
        )?;
        for (index, track) in mutable_track_revisions.iter().enumerate() {
            let PreparedMutableTrackRevision {
                id: _,
                instrument: _,
                instrument_position: _,
                effects,
                arrangement_midi_sources,
            } = track;
            report.vec(owner, "revision.effects", [index, 0], effects)?;
            report.vec(
                owner,
                "revision.midi_sources",
                [index, 0],
                arrangement_midi_sources,
            )?;
            // Device state and source identity internals remain covered by
            // revision.nested_owners Unknown, not presumed scalar-only.
        }
        Ok(())
    }
}

#[cfg(test)]
mod owned_storage_tests {
    use super::*;
    use crate::storage_report::{Report, StorageOwner, StorageReportError, owned_tests::Sink};
    #[test]
    fn descriptor_nested_backings_exclude_headers_and_preserve_refusal_owners() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let mut revision = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap();
        // A private accounting-only fixture exercises empty spare nested buffers.
        // It is never published or used to bypass preparation validation.
        revision.mutable_track_revisions = Vec::with_capacity(7);
        revision
            .mutable_track_revisions
            .push(PreparedMutableTrackRevision {
                id: track,
                instrument: None,
                instrument_position: None,
                effects: Vec::with_capacity(11),
                arrangement_midi_sources: Vec::with_capacity(13),
            });
        let outer = revision.mutable_track_revisions.capacity();
        let effects = revision.mutable_track_revisions[0].effects.capacity();
        let sources = revision.mutable_track_revisions[0]
            .arrangement_midi_sources
            .capacity();
        let revision = Arc::new(revision);
        let strong = Arc::strong_count(&revision);
        let mut sink = Sink::new(16);
        let mut report = Report::new(&mut sink);
        let owner = report.arc(&revision, "prepared_revision").unwrap();
        revision.visit_owned_storage(&mut report, owner).unwrap();
        assert!(report.finish().require_complete().is_err());
        for (name, capacity, stride) in [
            (
                "revision.mutable_tracks",
                outer,
                std::mem::size_of::<PreparedMutableTrackRevision>(),
            ),
            (
                "revision.effects",
                effects,
                std::mem::size_of::<PreparedDeviceIdentity>(),
            ),
            (
                "revision.midi_sources",
                sources,
                std::mem::size_of::<PreparedArrangementMidiSource>(),
            ),
        ] {
            assert_eq!(
                sink.measured(name, [0; 2]),
                (capacity, stride, capacity * stride)
            );
        }
        assert!(
            sink.items
                .iter()
                .all(|item| matches!(item.owner, StorageOwner::Shared(_)))
        );
        assert_eq!(Arc::strong_count(&revision), strong);
        drop(sink);
        for limit in 0..3 {
            let mut sink = Sink::new(limit);
            let mut report = Report::new(&mut sink);
            let owner = StorageOwner::Shared(crate::storage_report::SharedOwnerRef::new(
                &revision,
                "prepared_revision",
            ));
            assert_eq!(
                revision.visit_owned_storage(&mut report, owner),
                Err(StorageReportError::SinkCapacity)
            );
            assert_eq!(revision.mutable_track_revisions.len(), 1);
            assert_eq!(Arc::strong_count(&revision), strong);
        }
    }
}
