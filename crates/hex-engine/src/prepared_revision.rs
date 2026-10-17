//! Exact control-side products for one accepted canonical project revision.
use crate::{
    Invalid, MidiArrangementPreview, PreparedSessionInventory, Project as PlaybackProject,
    compile::PreparedAudioSource,
};
use hex_devices::SineTable;
pub(crate) mod assembly;
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
            state: device.state().clone().without_host_gains(),
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

static NEXT_TUNING_FAMILY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TuningContinuationFamily(u64);
impl TuningContinuationFamily {
    pub(crate) fn fresh() -> Result<Self, Invalid> {
        use std::sync::atomic::Ordering;
        NEXT_TUNING_FAMILY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(Self)
            .map_err(|_| Invalid("tuning continuation family exhausted"))
    }
}

/// Immutable compiled outputs prepared before a canonical revision is committed.
/// Arrangement playback and Session inventory share a stable instrument table;
/// runtime route ownership is maintained by the playback kernel.
pub struct PreparedRevision {
    tuning_family: TuningContinuationFamily,
    arrangement: crate::owned_shared::OwnedShared<PlaybackProject>,
    session: crate::owned_shared::OwnedShared<PreparedSessionInventory>,
    midi_preview: crate::owned_shared::OwnedShared<MidiArrangementPreview>,
    mutable_track_revisions: Vec<PreparedMutableTrackRevision>,
    gain_recording_track: Option<TrackId>,
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

    /// Prepare a tuning-only publication in the authoritative Session's existing
    /// project generation. The caller must pair `previous_project` with its actual
    /// accepted `previous_revision`; this API does not authenticate arbitrary caller
    /// metadata. It independently checks the exact supplied canonical delta and the
    /// prior compiled keyboard-relevant instrument/source/chain/timing/tuning facts.
    /// Opens and ordinary edits must use ordinary preparation, which starts a fresh
    /// family. Skipped tuning candidates can then continue the actual installed
    /// family member without retaining source graphs or replaying intermediate edits.
    pub fn prepare_tuning_from(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_project: &Project,
        previous_revision: &Self,
    ) -> Result<Self, Invalid> {
        Self::prepare(
            project,
            media,
            sine_table,
            Some(previous_revision.session()),
        )?
        .with_tuning_continuation_from(project, previous_project, previous_revision)
    }
    /// Consume an unpublished prepared candidate and verify the same tuning-only
    /// proof without repeating PCM/waveform preparation. No raw family setter exists.
    pub fn with_tuning_continuation_from(
        mut self,
        project: &Project,
        previous_project: &Project,
        previous_revision: &Self,
    ) -> Result<Self, Invalid> {
        if !previous_project.same_except_tuning(project)
            || self.arrangement.shared_contexts().tuning()
                != &project.tuning().table().map_err(|e| Invalid(e.0))?
            || previous_revision.arrangement.sample_rate() != previous_project.sample_rate()
            || previous_revision.arrangement.shared_contexts().tuning()
                != &previous_project
                    .tuning()
                    .table()
                    .map_err(|e| Invalid(e.0))?
            || !previous_revision
                .session
                .timing()
                .matches_canonical(previous_project.tempo_map())
            || previous_revision.session.meter_map() != previous_project.meter_map()
            || prepare_mutable_track_revisions(previous_project, &previous_revision.arrangement)?
                != previous_revision.mutable_track_revisions
            || previous_project
                .tracks()
                .iter()
                .zip(previous_revision.arrangement.tracks())
                .any(|(authored, compiled)| {
                    compiled.canonical_track_id() != Some(authored.id())
                        || compiled.instrument_id != authored.instrument().map(Device::id)
                        || compiled.instrument_state
                            != authored
                                .instrument()
                                .map(|device| device.state().clone().without_host_gains())
                        || compiled.instrument_bypassed
                            != authored.instrument().is_some_and(Device::bypassed)
                        || compiled.instrument_position
                            != authored.instrument_position().unwrap_or(0)
                })
        {
            return Err(Invalid(
                "keyboard tuning publication must preserve the exact project source",
            ));
        }
        if !keyboard_source_facts_match(previous_revision, &self) {
            return Err(Invalid(
                "keyboard tuning publication changed a prepared source or chain",
            ));
        }
        self.tuning_family = previous_revision.tuning_family;
        Ok(self)
    }
    pub(crate) fn tuning_family(&self) -> TuningContinuationFamily {
        self.tuning_family
    }

    /// Arm one transient raw-fader target without editing canonical metadata or history.
    /// Active processors must declare a known, bounded algorithmic latency.
    /// Playback reports the actual fader content origin and remaining delay to Main.
    pub fn prepare_for_gain_recording(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        track: TrackId,
    ) -> Result<Self, Invalid> {
        Self::prepare(project, media, sine_table, previous_session)?
            .arm_gain_recording(project, track)
    }
    /// Prepare transient recording with the control worker's existing transform lifecycle.
    pub fn prepare_for_gain_recording_with_audio_transforms(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        track: TrackId,
        transforms: &crate::audio_transform::AudioTransformPreparation,
    ) -> Result<Self, Invalid> {
        Self::prepare_with_audio_transforms(
            project,
            media,
            sine_table,
            previous_session,
            transforms,
        )?
        .arm_gain_recording(project, track)
    }

    /// Test registry seam for callback probes; production arming uses canonical preparation.
    #[cfg(feature = "instrument-test-support")]
    pub fn with_test_gain_recording(
        self,
        project: &Project,
        track: TrackId,
    ) -> Result<Self, Invalid> {
        self.arm_gain_recording(project, track)
    }

    pub(crate) fn arm_gain_recording(
        mut self,
        project: &Project,
        track: TrackId,
    ) -> Result<Self, Invalid> {
        let canonical = project
            .track(track)
            .ok_or(Invalid("gain recording track no longer exists"))?;
        let prepared = &mut self;
        prepared
            .arrangement
            .arm_gain_recording(track, canonical.gain(), canonical.balance())?;
        prepared.gain_recording_track = Some(track);
        Ok(self)
    }

    pub fn gain_recording_track(&self) -> Option<TrackId> {
        self.gain_recording_track
    }

    pub(crate) fn prepare_with_instrument_registry(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, Invalid> {
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        Self::prepare_with_registries(
            project,
            media,
            sine_table,
            previous_session,
            &effects,
            instruments,
        )
    }

    pub(crate) fn prepare_with_registries(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, Invalid> {
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        Self::prepare_with_registries_and_transforms(
            project,
            media,
            sine_table,
            previous_session,
            effects,
            instruments,
            &transforms,
        )
    }

    pub fn prepare_with_audio_transforms(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
    ) -> Result<Self, Invalid> {
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_registries_and_transforms(
            project,
            media,
            sine_table,
            previous_session,
            &effects,
            &instruments,
            transforms,
        )
    }

    fn prepare_with_registries_and_transforms(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
    ) -> Result<Self, Invalid> {
        transforms.compile_transaction(|| {
            crate::gain_envelope::validate_project(project)?;
            crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
            let shared_contexts = crate::shared_analytic::SharedAnalyticContextTable::prepare(
                project,
                instruments,
                previous_session.map(|previous| previous.shared_contexts().as_ref()),
                None,
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
                    effects,
                    instruments,
                    Some(shared_contexts.clone_on_control()),
                    Some(transforms),
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
                .map_err(|_| {
                    Invalid("active effect and mutable instrument runtime budget exceeded")
                })?;
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
                tuning_family: TuningContinuationFamily::fresh()?,
                arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
                session: crate::owned_shared::OwnedShared::new_owned(session),
                midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
                mutable_track_revisions,
                gain_recording_track: None,
            })
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

    /// Test-only registry injection through the complete production preparation path.
    #[cfg(feature = "instrument-test-support")]
    #[doc(hidden)]
    pub fn prepare_with_test_registries(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        table: Arc<SineTable>,
        previous: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
    ) -> Result<Self, Invalid> {
        Self::prepare_with_registries(project, media, table, previous, effects, instruments)
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
            None,
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
                None,
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
            tuning_family: TuningContinuationFamily::fresh()?,
            arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
            session: crate::owned_shared::OwnedShared::new_owned(session),
            midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
            mutable_track_revisions,
            gain_recording_track: None,
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
        self.arrangement.shared_contexts().tuning_identity()
            == other.arrangement.shared_contexts().tuning_identity()
            && old.topology_matches(new)
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
        self.arrangement.shared_contexts().tuning_identity()
            == other.arrangement.shared_contexts().tuning_identity()
            && old.topology_matches(new)
            && old.arrangement_sources_match(new)
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
    // Keep bounded source/topology evidence for providers that authorize a
    // future analytic import as well as for existing mutable owners.
    let needs_descriptor = |track: &crate::Track| {
        track.has_mutable_instrument()
            || track
                .instrument_id
                .and_then(|device| arrangement.shared_contexts().owner_for_device(device))
                .is_some_and(|owner| {
                    owner
                        .analytic_context()
                        .is_some_and(|context| context.import_identity().is_some())
                })
    };
    let mutable_track_count = arrangement
        .tracks()
        .iter()
        .filter(|track| needs_descriptor(track))
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
        if !needs_descriptor(compiled) {
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

        let source_id = project.midi_source_track(track.id());
        let source = project
            .track(source_id)
            .ok_or(Invalid("mutable midi source root is missing"))?;
        let mut arrangement_midi_sources = Vec::new();
        arrangement_midi_sources
            .try_reserve_exact(source.midi_clips().len())
            .map_err(|_| Invalid("could not reserve mutable midi source descriptors"))?;
        arrangement_midi_sources.extend(source.midi_clips().iter().map(|clip| {
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

fn keyboard_source_facts_match(source: &PreparedRevision, target: &PreparedRevision) -> bool {
    source.arrangement.tracks().len() == target.arrangement.tracks().len()
        && source
            .arrangement
            .tracks()
            .iter()
            .zip(target.arrangement.tracks())
            .all(|(a, b)| {
                a.track_id == b.track_id
                    && a.instrument_id == b.instrument_id
                    && a.instrument_state == b.instrument_state
                    && a.instrument_bypassed == b.instrument_bypassed
                    && a.instrument_position == b.instrument_position
                    && a.instrument_input_gain == b.instrument_input_gain
                    && a.instrument_output_gain == b.instrument_output_gain
                    && a.midi_source_index == b.midi_source_index
                    && a.device_chain.len() == b.device_chain.len()
                    && a.device_chain
                        .iter()
                        .zip(b.device_chain.iter())
                        .all(|(a, b)| {
                            a.owner == b.owner
                                && a.device_id == b.device_id
                                && a.device_type == b.device_type
                                && a.bypassed == b.bypassed
                                && a.sample_rate == b.sample_rate
                                && a.matrix == b.matrix
                                && match (&a.configuration, &b.configuration) {
                                    (Some(a), Some(b)) => a.processing_eq(b),
                                    (None, None) => true,
                                    _ => false,
                                }
                        })
                    && match (&a.instrument, &b.instrument) {
                        (None, None) => true,
                        (Some(a), Some(b)) => {
                            a.spans.len() == b.spans.len()
                                && a.spans.iter().zip(&b.spans).all(|(a, b)| {
                                    (
                                        a.start,
                                        a.gate_end,
                                        a.onset_beat,
                                        a.gate_end_beat,
                                        a.pitch,
                                        a.channel,
                                        a.velocity,
                                        a.clip,
                                        a.note,
                                        a.iteration,
                                    ) == (
                                        b.start,
                                        b.gate_end,
                                        b.onset_beat,
                                        b.gate_end_beat,
                                        b.pitch,
                                        b.channel,
                                        b.velocity,
                                        b.clip,
                                        b.note,
                                        b.iteration,
                                    )
                                })
                        }
                        _ => false,
                    }
            })
        && source.session.tuning_source_facts_match(&target.session)
}
