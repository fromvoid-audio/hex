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
    arrangement: crate::owned_shared::OwnedShared<PlaybackProject>,
    session: crate::owned_shared::OwnedShared<PreparedSessionInventory>,
    midi_preview: crate::owned_shared::OwnedShared<MidiArrangementPreview>,
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
                Some(shared_contexts.clone_on_control()),
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
            arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
            session: crate::owned_shared::OwnedShared::new_owned(session),
            midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
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
                Some(shared_contexts.clone_on_control()),
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
            arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
            session: crate::owned_shared::OwnedShared::new_owned(session),
            midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
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

impl PreparedRevision {
    pub fn into_owned_shared(self) -> crate::OwnedPreparedRevision {
        crate::OwnedPreparedRevision::new(self)
    }
    pub(crate) fn arrangement_owner(&self) -> &crate::owned_shared::OwnedShared<PlaybackProject> {
        &self.arrangement
    }
    pub(crate) fn session_owner(
        &self,
    ) -> &crate::owned_shared::OwnedShared<PreparedSessionInventory> {
        &self.session
    }
    pub(crate) fn preview_owner(
        &self,
    ) -> &crate::owned_shared::OwnedShared<MidiArrangementPreview> {
        &self.midi_preview
    }
}

#[cfg(test)]
mod owned_ingress_tests {
    use super::*;
    use crate::{
        owned_shared::OwnedShared,
        playback::{Playback, PreparedOrigin},
    };
    #[test]
    fn owned_ingress_recovers_exact_owner_and_receipt_on_preflight_refusal() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "track".into(),
            })
            .unwrap();
        let mut value = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap();
        // Accounting-only invalid fixture, never published: force the existing
        // exact Arrangement/Session track mismatch before any root clone.
        value.arrangement = OwnedShared::new_owned(PlaybackProject::new(48_000, vec![]).unwrap());
        let owner = value.into_owned_shared();
        let before = owner.owner.parts();
        let alias = owner.clone_on_control();
        assert_eq!(Arc::strong_count(owner.owner.arc()), 2);
        crate::owned_shared::reset_clone_count();
        let Err((error, returned)) = Playback::try_from_owned_prepared_with_session(owner) else {
            panic!("expected mismatch")
        };
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert_eq!(error, crate::SessionPlaybackError::RevisionTrackMismatch);
        assert!(Arc::ptr_eq(returned.owner.arc(), alias.owner.arc()));
        assert_eq!(returned.owner.parts(), before);
        assert_eq!(Arc::strong_count(returned.owner.arc()), 2);
        assert_eq!(returned.metadata().sample_rate(), 48_000);
        assert!(!returned.metadata().contains_streamed_sources());
    }
    #[test]
    fn owned_metadata_and_playback_identity_do_not_expose_the_legacy_arc() {
        let canonical = Project::new(48_000).unwrap();
        let make = || {
            PreparedRevision::prepare(
                &canonical,
                &BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap()
        };
        let owner = make().into_owned_shared();
        let alias = owner.clone_on_control();
        let independent = make().into_owned_shared();
        let playback = Playback::try_from_owned_prepared_with_session(owner)
            .ok()
            .unwrap();
        assert_eq!(playback.prepared_origin(), PreparedOrigin::OwnedMetadata);
        assert!(playback.prepared_revision().is_none());
        assert_eq!(playback.owned_metadata().unwrap().sample_rate(), 48_000);
        assert!(playback.same_prepared_owner(&alias));
        assert!(!playback.same_prepared_owner(&independent));
        assert_eq!(Arc::strong_count(alias.owner.arc()), 3); // Runtime, Playback, tracked alias.
        let mut playback = playback;
        crate::owned_shared::reset_clone_count();
        playback.apply(crate::playback::PlaybackCommand::Play);
        let mut frames = [[0.0; 2]; 64];
        playback.render(&mut frames).unwrap();
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert!(playback.same_prepared_owner(&alias));
        assert_eq!(Arc::strong_count(alias.owner.arc()), 3);
        let positive = alias.clone_on_control();
        assert_eq!(crate::owned_shared::clone_count(), 1);
        drop(positive);
        let legacy = Playback::try_from_prepared_with_session(Arc::new(make())).unwrap();
        assert_eq!(legacy.prepared_origin(), PreparedOrigin::LegacyArc);
        assert!(legacy.prepared_revision().is_some());
        assert!(legacy.owned_metadata().is_none());
        assert!(!legacy.same_prepared_owner(&alias));
    }
}

#[cfg(test)]
mod owned_report_tests {
    use super::*;
    use crate::{
        playback::Playback,
        storage_report::{
            StorageCharge, StorageOwner, StorageReportError, UnknownStorageReason,
            owned_tests::Sink,
        },
    };
    #[test]
    fn paired_root_parent_sizes_alias_dedup_and_every_partial_refusal_are_truthful() {
        let canonical = Project::new(48_000).unwrap();
        let owner = PreparedRevision::prepare(
            &canonical,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        let alias = owner.clone_on_control();
        let root_receipt = owner.owner.parts();
        let mut playback = Playback::try_from_owned_prepared_with_session(owner)
            .ok()
            .unwrap();
        let before = Arc::strong_count(alias.owner.arc());
        let mut sink = Sink::new(512);
        let summary = playback.visit_storage_on_control(&mut sink).unwrap();
        assert!(summary.require_complete().is_err());
        let roots: Vec<_> = sink.items.iter().filter(|item| item.location.component == "arc.pointee" && matches!(item.owner, StorageOwner::Shared(owner) if owner.category() == "prepared_revision")).collect();
        assert_eq!(roots.len(), 2); // Playback and its Session runtime alias one root.
        for item in &roots {
            assert!(
                matches!(item.charge, StorageCharge::MeasuredData { bytes, .. } if bytes == std::mem::size_of::<PreparedRevision>())
            );
        }
        let owner_ref = |index: usize| {
            let StorageOwner::Shared(owner) = roots[index].owner else {
                panic!("root")
            };
            owner
        };
        assert!(owner_ref(0).same_allocation(owner_ref(1)));
        assert!(sink.items.iter().any(|item| matches!(
            item.charge,
            StorageCharge::Unknown(UnknownStorageReason::RetirementReceipt)
        )));
        if let Some((payload, overhead)) = root_receipt {
            assert_eq!(payload, std::mem::size_of::<PreparedRevision>());
            assert!(sink.items.iter().any(|item| item.location.component == "arc.owner_overhead" && matches!(item.charge, StorageCharge::OwnerOverheadDeclared { bytes, .. } if bytes == overhead)));
        }
        assert_eq!(Arc::strong_count(alias.owner.arc()), before);
        let count = sink.items.len();
        drop(sink);
        for limit in 0..count {
            let mut sink = Sink::new(limit);
            assert_eq!(
                playback.visit_storage_on_control(&mut sink),
                Err(StorageReportError::SinkCapacity)
            );
            assert_eq!(Arc::strong_count(alias.owner.arc()), before);
            assert_eq!(playback.position(), 0);
            // No partial sink is reused as an admission receipt.
        }
    }
    #[test]
    fn restricted_metadata_queries_have_stable_ids_and_borrow_only_leaf_samples() {
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "TrackName".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let hex_project::Created::Scene(scene) = editor
            .execute(hex_project::Command::AddScene {
                name: "SceneName".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let root = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        let view = root.metadata();
        assert_eq!(view.track_ids().collect::<Vec<_>>(), [track]);
        assert_eq!(
            view.scene_names().collect::<Vec<_>>(),
            [(scene, "SceneName")]
        );
        assert_eq!(view.streamed_sources().count(), 0);
        assert_eq!(Arc::strong_count(root.owner.arc()), 1);
    }
}
