//! Exact control-side products for one accepted canonical project revision.
//!
//! Arrangement, Session inventory and display summaries are prepared together before the canonical
//! edit/history transaction commits. Preparation failure leaves the previous project, IDs, redo
//! and accepted audio source intact. Musical compatibility uses authored source/device identity,
//! not equal filenames or display metadata. A private resource_scope binds source-bearing sampler
//! coefficients to their sealed ControlSamplerSources sidecar; an ordinary runtime constructor
//! refuses that private graph. The separate device_resource_scope stores only the exact registered
//! resource-set identity: ordinary runtime construction must receive that matching set and retain
//! actual source leases before playback.
use crate::{
    Invalid, MidiArrangementPreview, PreparedSessionInventory, Project as PlaybackProject,
    compile::PreparedAudioSource,
};
use hex_devices::SineTable;
pub(crate) mod assembly;
mod authored_midi;
pub(crate) mod foreign_programs;
use hex_project::{
    AssetId, Device, DeviceId, MAX_CLIPS, MAX_DEVICES_PER_TRACK, MAX_TRACKS, MidiSourceIdentity,
    Project, TrackId,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, PartialEq, Eq)]
struct PreparedDeviceIdentity {
    id: DeviceId,
    parent: Option<DeviceId>,
    state: hex_lattice_contract::NativeDeviceState,
    bypassed: bool,
}

impl From<&Device> for PreparedDeviceIdentity {
    fn from(device: &Device) -> Self {
        Self {
            id: device.id(),
            parent: device.parent(),
            state: device.state().clone().without_host_gains(),
            bypassed: device.bypassed(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedArrangementMidiSource {
    content_view: Option<hex_project::ContentWindow>,
    active: bool,
    playback_offset: hex_project::BeatTime,
    id: hex_project::ClipId,
    start: hex_project::BeatTime,
    length: hex_project::BeatTime,
    end_frame: u64,
    source: MidiSourceIdentity,
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedMutableTrackRevision {
    id: TrackId,
    /// Supplemental evidence is visible only to the bounded source query.
    continuity_only: bool,
    midi_source_track: TrackId,
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

    /// The same beat-anchored sources, ignoring the frame each clip ends at.
    /// Only an edited base tempo map may move those frames (Pulse #4117).
    fn arrangement_sources_match_retimed(&self, other: &Self) -> bool {
        self.arrangement_midi_sources.len() == other.arrangement_midi_sources.len()
            && self
                .arrangement_midi_sources
                .iter()
                .zip(&other.arrangement_midi_sources)
                .all(|(old, new)| old.same_beats(new))
    }
}

impl PreparedArrangementMidiSource {
    fn same_beats(&self, other: &Self) -> bool {
        let Self {
            content_view,
            active,
            playback_offset,
            id,
            start,
            length,
            end_frame: _,
            source,
        } = self;
        *content_view == other.content_view
            && *active == other.active
            && *playback_offset == other.playback_offset
            && *id == other.id
            && *start == other.start
            && *length == other.length
            && *source == other.source
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
    source_routing: crate::PreparedSourceRouting,
    pub(crate) resource_scope: Option<u64>,
    pub(crate) device_resource_scope: Option<u64>,
    tuning_family: TuningContinuationFamily,
    arrangement: crate::owned_shared::OwnedShared<PlaybackProject>,
    session: crate::owned_shared::OwnedShared<PreparedSessionInventory>,
    midi_preview: crate::owned_shared::OwnedShared<MidiArrangementPreview>,
    mutable_track_revisions: Vec<PreparedMutableTrackRevision>,
    authored_midi_sources: Vec<authored_midi::Source>,
    gain_recording_track: Option<TrackId>,
    native_parameter_input: Option<crate::device_automation::NativeParameterInputTarget>,
    pub(crate) scalar_automation: Option<Box<[crate::scalar_automation::PreparedBinding]>>,
    pub(crate) foreign_automation: crate::foreign_automation::Programs,
}

impl std::fmt::Debug for PreparedRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRevision")
            .field("arrangement_rate", &self.arrangement.sample_rate())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// Prepare the authored and resource-backed graphs independently, then let
/// the admitted instrument program take sole ownership of its output trim.
fn assign_prepared_instrument_output_trims(
    arrangement: &mut PlaybackProject,
    session: &PreparedSessionInventory,
) {
    for (index, track) in arrangement.tracks.iter_mut().enumerate() {
        if track.instrument_output_gain != 1.0
            && track
                .instrument_id
                .is_some_and(|device| session.has_instrument_output_modulation_at(index, device))
        {
            track.instrument_output_gain = 1.0;
        }
    }
}

impl PreparedRevision {
    #[cfg(test)]
    pub(crate) fn test_install_source_effect_recipes(
        &mut self,
        recipes: Vec<crate::source_effect_stage::PreparedSourceEffects>,
    ) {
        let mut arrangement = self.arrangement().as_ref().clone();
        arrangement.source_effects = recipes;
        self.arrangement = crate::owned_shared::OwnedShared::new_owned(arrangement);
    }

    #[cfg(test)]
    pub(crate) fn test_install_retained_receiver(
        &mut self,
        canonical: &Project,
        track: TrackId,
    ) -> Result<(), Invalid> {
        let mut arrangement = self.arrangement().as_ref().clone();
        let index = arrangement
            .tracks
            .iter()
            .position(|row| row.canonical_track_id() == Some(track))
            .ok_or(Invalid("receiver track is missing"))?;
        arrangement.tracks[index].source_input_enabled = false;
        arrangement.tracks[index].instrument_output_gain = 0.5;
        self.source_routing
            .test_exclude_receiver_contribution(index)?;
        let graph =
            crate::signal_graph::SignalGraph::test_printed_branch(&arrangement, canonical, index)?;
        arrangement.main_output_latency_frames = crate::main_chain::output_latency(
            graph.main_latency(),
            &arrangement.main_device_chain,
        )?;
        arrangement.signal_graph = Some(graph);
        for row in &mut arrangement.tracks {
            row.source_mix = crate::PreparedTrackMix::identity();
        }
        self.arrangement = crate::owned_shared::OwnedShared::new_owned(arrangement);
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn test_install_group_graph(&mut self, canonical: &Project) {
        let mut arrangement = self.arrangement().as_ref().clone();
        arrangement.signal_graph =
            crate::signal_graph::SignalGraph::prepare(&arrangement, &[], canonical).unwrap();
        for track in &mut arrangement.tracks {
            track.source_mix = crate::PreparedTrackMix::identity();
        }
        self.arrangement = crate::owned_shared::OwnedShared::new_owned(arrangement);
    }

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

    /// Prepare one recorded normalized source through the same native factory
    /// union as live input. Delayed stages use the graph's transport history.
    /// Metadata alone grants no page lease or native publication authority.
    pub fn prepare_with_scalar_automation(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        binding: crate::scalar_automation::ScalarAutomationBinding,
    ) -> Result<Self, Invalid> {
        let live = Some((binding.device, binding.target));
        Self::prepare_with_scalar_automation_sources(
            project,
            crate::compile::SourceBank::Resident(media),
            sine_table,
            previous_session,
            vec![binding],
            live,
        )
    }

    /// Same recorded target and factory admission with prepared PCM sources.
    pub fn prepare_with_scalar_automation_and_prepared_sources(
        project: &Project,
        sources: &BTreeMap<AssetId, crate::compile::PreparedAudioSource>,
        source_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        binding: crate::scalar_automation::ScalarAutomationBinding,
    ) -> Result<Self, Invalid> {
        let live = Some((binding.device, binding.target));
        Self::prepare_with_scalar_automation_bindings(
            project,
            sources,
            source_generation,
            sine_table,
            previous_session,
            vec![binding],
            live,
        )
    }

    /// Prepare a bounded group of recorded targets against the actual stage
    /// origins and one optional, independent live-input arm. Source metadata
    /// grants no page lease or native publication authority.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_scalar_automation_bindings(
        project: &Project,
        sources: &BTreeMap<AssetId, crate::compile::PreparedAudioSource>,
        source_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        bindings: Vec<crate::scalar_automation::ScalarAutomationBinding>,
        live: Option<(
            hex_project::DeviceId,
            hex_lattice_contract::DeviceAutomationTarget,
        )>,
    ) -> Result<Self, Invalid> {
        Self::prepare_with_scalar_automation_sources(
            project,
            crate::compile::SourceBank::Mixed(sources, source_generation),
            sine_table,
            previous_session,
            bindings,
            live,
        )
    }

    fn prepare_with_scalar_automation_sources<'a>(
        project: &'a Project,
        sources: crate::compile::SourceBank<'a>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        mut bindings: Vec<crate::scalar_automation::ScalarAutomationBinding>,
        live: Option<(
            hex_project::DeviceId,
            hex_lattice_contract::DeviceAutomationTarget,
        )>,
    ) -> Result<Self, Invalid> {
        use crate::scalar_automation::{MAX_SCALAR_AUTOMATION_BINDINGS, PreparedBinding};

        if !(1..=MAX_SCALAR_AUTOMATION_BINDINGS).contains(&bindings.len()) {
            return Err(Invalid("scalar automation binding count exceeds its limit"));
        }
        bindings.sort_unstable_by_key(|binding| (binding.device, binding.target));
        if bindings
            .windows(2)
            .any(|pair| (pair[0].device, pair[0].target) == (pair[1].device, pair[1].target))
        {
            return Err(Invalid("duplicate scalar automation target"));
        }
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(bindings.len())
            .map_err(|_| Invalid("scalar binding allocation failed"))?;
        for (index, binding) in bindings.iter().enumerate() {
            if bindings[..index]
                .iter()
                .any(|old| old.source.key() == binding.source.key())
            {
                return Err(Invalid("duplicate scalar automation source key"));
            }
            crate::scalar_automation::validate_source(project, binding)?;
            targets.push(crate::device_automation::native_parameter_input_target(
                project,
                binding.device,
                binding.target,
            )?);
        }
        let live = live
            .map(|(device, target)| {
                crate::device_automation::native_parameter_input_target(project, device, target)
            })
            .transpose()?;
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        let mut prepared = Self::prepare_with_source_bank(
            project,
            sources,
            sine_table,
            previous_session,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            live,
            &targets,
        )?;
        let graph = prepared
            .arrangement
            .signal_graph
            .as_ref()
            .ok_or(Invalid("scalar signal graph is unavailable"))?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(bindings.len())
            .map_err(|_| Invalid("scalar binding allocation failed"))?;
        for (request, input) in bindings.into_iter().zip(targets) {
            result.push(PreparedBinding {
                request,
                input,
                arrival_frames: graph.parameter_arrival(input)?,
            });
        }
        for binding in &result {
            prepared
                .arrangement
                .prepare_scalar_automation(binding.input)?;
        }
        prepared.scalar_automation = Some(result.into_boxed_slice());
        Ok(prepared)
    }

    pub fn scalar_automation_binding(
        &self,
    ) -> Option<&crate::scalar_automation::ScalarAutomationBinding> {
        self.scalar_automation_bindings().next()
    }

    pub fn scalar_automation_bindings(
        &self,
    ) -> impl Iterator<Item = &crate::scalar_automation::ScalarAutomationBinding> {
        self.scalar_automation
            .as_deref()
            .into_iter()
            .flatten()
            .map(|binding| &binding.request)
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
        if self.scalar_automation.is_some()
            || previous_revision.scalar_automation.is_some()
            || !previous_project.same_except_tuning(project)
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
            || prepare_mutable_track_revisions(
                previous_project,
                &previous_revision.arrangement,
                previous_revision.device_resource_scope.is_some(),
            )? != previous_revision.mutable_track_revisions
            || (previous_revision.device_resource_scope.is_some()
                && authored_midi::prepare(previous_project)?
                    != previous_revision.authored_midi_sources)
            || previous_project
                .tracks()
                .iter()
                .zip(previous_revision.arrangement.tracks())
                .any(|(authored, compiled)| {
                    // Every contributor retains complete canonical descriptors
                    // above; only the receiver owns compiled generator fields.
                    let instrument = authored
                        .instrument()
                        .filter(|_| authored.receives_source_audio());
                    compiled.canonical_track_id() != Some(authored.id())
                        || compiled.instrument_id != instrument.map(Device::id)
                        || compiled.instrument_state
                            != instrument.map(|device| device.state().clone().without_host_gains())
                        || compiled.instrument_bypassed != instrument.is_some_and(Device::bypassed)
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

    /// Consume an unshared actual compiled graph to arm one transient raw fader. The control
    /// caller supplies its matching canonical project; no source/factory ownership is adopted
    /// or recompiled, and an already shared arrangement refuses before mutation.
    pub fn arm_gain_recording(
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

    /// Prepare one transient native input through the ordinary compiler, without
    /// creating canonical points or acquiring a second runtime/publication owner.
    pub fn prepare_for_native_parameter_input(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        device: hex_project::DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
    ) -> Result<Self, Invalid> {
        Self::prepare_for_native_parameter_input_with_audio_transforms(
            project,
            media,
            sine_table,
            previous_session,
            device,
            target,
            &crate::audio_transform::AudioTransformPreparation::default(),
        )
    }
    /// Same transient input with the control worker's existing transform lifecycle.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_for_native_parameter_input_with_audio_transforms(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        device: hex_project::DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
        transforms: &crate::audio_transform::AudioTransformPreparation,
    ) -> Result<Self, Invalid> {
        let input =
            crate::device_automation::native_parameter_input_target(project, device, target)?;
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_source_bank(
            project,
            crate::compile::SourceBank::Resident(media),
            sine_table,
            previous_session,
            &effects,
            &instruments,
            transforms,
            None,
            Some(input),
            &[],
        )
    }
    /// Exact source locator armed during preparation, never a live application ACK.
    pub fn native_parameter_input_target(
        &self,
    ) -> Option<crate::device_automation::NativeParameterInputTarget> {
        self.native_parameter_input
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

    /// Prepare the ordinary resident graph with a host's effect registry and
    /// the existing built-in instrument registry. Foreign factories declare
    /// immutable facts only; the audio host must separately retain and admit
    /// their actual source/worker/endpoint owners. This grants no publication
    /// authority and never reconstructs a native instance from state bytes.
    pub fn prepare_with_effect_registry(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
    ) -> Result<Self, Invalid> {
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_registries(
            project,
            media,
            sine_table,
            previous_session,
            effects,
            &instruments,
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
        Self::prepare_with_registries_and_sampler_sources(
            project,
            media,
            sine_table,
            previous_session,
            effects,
            instruments,
            transforms,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_with_registries_and_sampler_sources(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
    ) -> Result<Self, Invalid> {
        Self::prepare_with_source_bank(
            project,
            crate::compile::SourceBank::Resident(media),
            sine_table,
            previous_session,
            effects,
            instruments,
            transforms,
            samplers,
            None,
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_with_source_bank<'a>(
        project: &'a Project,
        sources: crate::compile::SourceBank<'a>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        instruments: &hex_lattice::NativeInstrumentRegistry<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        samplers: Option<&dyn crate::native_device_resources::InstrumentResourcePreparation>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
        external_inputs: &[crate::device_automation::NativeParameterInputTarget],
    ) -> Result<Self, Invalid> {
        transforms.compile_transaction(|| {
            crate::gain_envelope::validate_project(project)?;
            crate::return_feed_plan::ReturnFeedPlan::validate_admission(project)?;
            let shared_contexts =
                crate::shared_analytic::SharedAnalyticContextTable::prepare_scoped(
                    project,
                    instruments,
                    previous_session.map(|previous| previous.shared_contexts().as_ref()),
                    samplers,
                )?;
            if shared_contexts.payload_bytes()
                > crate::shared_analytic::MAX_SHARED_CONTEXT_BYTES_PER_REVISION
            {
                return Err(Invalid(
                    "shared instrument context revision budget exceeded",
                ));
            }
            let (mut arrangement, midi_preview) = crate::compile::project_with_source_bank(
                project,
                sources,
                Some(Arc::clone(&sine_table)),
                effects,
                instruments,
                Some(Arc::clone(&shared_contexts)),
                Some(transforms),
                samplers,
                native_input,
                external_inputs,
            )?;
            let session = PreparedSessionInventory::prepare_with_source_bank(
                project,
                sources,
                sine_table,
                previous_session,
                instruments,
                shared_contexts,
                samplers,
                native_input,
            )?;
            assign_prepared_instrument_output_trims(&mut arrangement, &session);
            session.validate_shared_source_capacity(project, &arrangement)?;
            let source_routing = crate::PreparedSourceRouting::prepare_for_playback(
                project,
                &arrangement,
                &session,
            )?;
            crate::mutable_instrument::validate_prepared_graph_budget(&arrangement, &session)
                .map_err(|_| {
                    Invalid("active effect and mutable instrument runtime budget exceeded")
                })?;
            let mutable_track_revisions = prepare_mutable_track_revisions(
                project,
                &arrangement,
                samplers
                    .and_then(|sources| sources.device_scope())
                    .is_some(),
            )?;
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
                source_routing,
                resource_scope: samplers.and_then(|sources| sources.private_scope()),
                device_resource_scope: samplers.and_then(|sources| sources.device_scope()),
                tuning_family: TuningContinuationFamily::fresh()?,
                arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
                session: crate::owned_shared::OwnedShared::new_owned(session),
                midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
                mutable_track_revisions,
                authored_midi_sources: if samplers
                    .and_then(|sources| sources.device_scope())
                    .is_some()
                    || authored_midi::has_inactive(project)
                {
                    authored_midi::prepare(project)?
                } else {
                    Vec::new()
                },
                gain_recording_track: None,
                native_parameter_input: native_input,
                scalar_automation: None,
                foreign_automation: Default::default(),
            })
        })
    }

    /// Compile through the ordinary registered-instrument path against exact original-source
    /// configurations. The returned revision owns no PCM; playback construction must supply the
    /// same set. The caller reserves every factory's two full PCM scans, including repeated
    /// Arrangement/Session/shared-context preparation, with no hidden precomputed sampler cache.
    /// This is aggregate compiler frame work, not maximum source extent: an enabled instrument
    /// currently has three factory invocations, each with two PCM scans (two invocations when
    /// bypassed). Consumers must reserve this separate bounded work without reducing or increasing
    /// their original resident import/storage extent limit.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_device_resources(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        max_scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Self, Invalid> {
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        Self::prepare_with_device_resources_and_effect_registry(
            project,
            media,
            sine_table,
            previous_session,
            transforms,
            resources,
            max_scan_frames,
            cancel,
            &effects,
            None,
        )
    }

    /// The same registered-resource compiler with host-provided effect
    /// definitions. Original sampler leases, transformations, cancellation and
    /// scan reservations retain their existing owners and validation. Foreign
    /// facts grant no native source/endpoint or publication custody; the host
    /// must separately retain the matching actual instances through delivery.
    #[allow(clippy::too_many_arguments)] // Existing complete resident inputs plus the effect registry.
    pub fn prepare_with_device_resources_and_effect_registry(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        max_scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<Self, Invalid> {
        resources.validate_project(project)?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        let preparation = crate::native_device_resources::CompilerDeviceResources::new(
            resources,
            max_scan_frames,
            cancel,
        );
        Self::prepare_with_source_bank(
            project,
            crate::compile::SourceBank::Resident(media),
            sine_table,
            previous_session,
            effects,
            &instruments,
            transforms,
            Some(&preparation),
            native_input,
            &[],
        )
    }

    /// Prepare mixed resident/file-backed clips through the full ordinary native-resource
    /// compiler. Resource identity, factory scans, authored MIDI and runtime budgets retain
    /// their ordinary contracts. Source facts alone grant no page readiness or publication.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_prepared_audio_sources_and_device_resources(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        max_scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Self, Invalid> {
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        Self::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
            project,
            sources,
            expected_generation,
            sine_table,
            previous_session,
            transforms,
            resources,
            max_scan_frames,
            cancel,
            &effects,
            None,
        )
    }

    /// Same mixed-file/resource compiler with host-borrowed effect facts.
    /// Original file catalogs, sampler leases and all input validation remain
    /// required. Registry facts confer no native source, page or publication
    /// custody; the host must retain the actual matching native instances.
    #[allow(clippy::too_many_arguments)] // Complete existing mixed inputs plus borrowed effect definitions.
    pub fn prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        max_scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<Self, Invalid> {
        if expected_generation == 0 {
            return Err(Invalid("stream source generation must be positive"));
        }
        resources.validate_project(project)?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        let preparation = crate::native_device_resources::CompilerDeviceResources::new(
            resources,
            max_scan_frames,
            cancel,
        );
        Self::prepare_with_source_bank(
            project,
            crate::compile::SourceBank::Mixed(sources, expected_generation),
            sine_table,
            previous_session,
            effects,
            &instruments,
            transforms,
            Some(&preparation),
            native_input,
            &[],
        )
    }

    /// Control-only scalar compilation against an exact sealed source sidecar.
    /// The returned graph owns no source bank; generic runtime constructors
    /// refuse it without the matching sidecar. Applications must keep it private.
    pub fn prepare_control_sampler_export(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        sources: &crate::native_device_resources::ControlSamplerSources,
    ) -> Result<Self, Invalid> {
        sources.validate_project(project)?;
        let effects = hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?;
        let instruments = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
        Self::prepare_with_registries_and_sampler_sources(
            project,
            media,
            sine_table,
            None,
            &effects,
            &instruments,
            transforms,
            Some(sources),
        )
    }
    /// Whether playback/export must receive the exact ordinary resource set used by compilation.
    /// This scalar grants no source identity, owning handle or publication/admission authority.
    pub fn requires_device_resources(&self) -> bool {
        self.device_resource_scope.is_some()
    }
    pub fn requires_control_sampler_sources(&self) -> bool {
        self.resource_scope.is_some()
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
        )?;
        if shared_contexts.payload_bytes()
            > crate::shared_analytic::MAX_SHARED_CONTEXT_BYTES_PER_REVISION
        {
            return Err(Invalid(
                "shared instrument context revision budget exceeded",
            ));
        }
        let (mut arrangement, midi_preview) =
            crate::compile::project_with_prepared_audio_sources_and_midi_preview_and_contexts(
                project,
                sources,
                expected_generation,
                Some(Arc::clone(&sine_table)),
                Some(Arc::clone(&shared_contexts)),
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
        assign_prepared_instrument_output_trims(&mut arrangement, &session);
        session.validate_shared_source_capacity(project, &arrangement)?;
        let source_routing =
            crate::PreparedSourceRouting::prepare_for_playback(project, &arrangement, &session)?;
        crate::mutable_instrument::validate_prepared_graph_budget(&arrangement, &session)
            .map_err(|_| Invalid("active effect and mutable instrument runtime budget exceeded"))?;
        let mutable_track_revisions =
            prepare_mutable_track_revisions(project, &arrangement, false)?;
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
            source_routing,
            resource_scope: None,
            device_resource_scope: None,
            tuning_family: TuningContinuationFamily::fresh()?,
            arrangement: crate::owned_shared::OwnedShared::new_owned(arrangement),
            session: crate::owned_shared::OwnedShared::new_owned(session),
            midi_preview: crate::owned_shared::OwnedShared::new_owned(midi_preview),
            mutable_track_revisions,
            authored_midi_sources: Vec::new(),
            gain_recording_track: None,
            native_parameter_input: None,
            scalar_automation: None,
            foreign_automation: Default::default(),
        })
    }

    /// Install preflighted authored programs before publishing this revision.
    /// The preparation owner must prove the program bounds for the transport
    /// profile and ensure the original armed worker starts with this program
    /// at a physical quantum boundary. A refusal leaves this revision intact.
    pub fn install_foreign_automation_on_control(
        &mut self,
        programs: Vec<crate::foreign_automation::BoundProgram>,
        reserved_bytes: usize,
    ) -> Result<(), Invalid> {
        if !self.foreign_automation.is_empty() {
            return Err(Invalid("foreign automation is already prepared"));
        }
        self.foreign_automation = crate::foreign_automation::Programs::prepare(
            programs,
            self.arrangement.as_ref(),
            reserved_bytes,
        )?;
        Ok(())
    }

    /// Immutable contributor/receiver metadata; no independent DSP owner.
    pub fn source_routing(&self) -> &crate::PreparedSourceRouting {
        &self.source_routing
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

    /// Borrow the actual immutable graph dependencies on control. Repeated
    /// bindings may appear in multiple clips; no allocation, clone or custody
    /// transfer occurs. Hosts must match these facts against retained catalogs.
    pub fn streamed_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.arrangement
            .streamed_sources()
            .chain(
                self.session
                    .audio_clips()
                    .iter()
                    .filter_map(|clip| match clip.source() {
                        crate::compile::PreparedAudioSource::Streamed(source) => {
                            Some(source.as_ref())
                        }
                        crate::compile::PreparedAudioSource::Resident(_) => None,
                    }),
            )
    }
    /// Borrow the exact source metadata retained by compiled Arrangement clips.
    /// Duplicate clip references may repeat a source; Session-only clips and
    /// unused asset records are excluded. This inventory includes inactive clip
    /// entries and grants no file freshness, page readiness or source custody.
    /// Offline consumers must budget/deduplicate it and prepare each real demand.
    pub fn arrangement_stream_sources(
        &self,
    ) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.arrangement.tracks().iter().flat_map(|track| {
            track.clips.iter().filter_map(|clip| {
                clip.metadata
                    .as_ref()
                    .and_then(|metadata| metadata.streamed.as_deref())
            })
        })
    }

    pub fn contains_streamed_sources(&self) -> bool {
        self.scalar_automation.is_some()
            || self.arrangement.contains_streamed_sources()
            || self.session.contains_streamed_sources()
    }

    /// Whether the compiled Arrangement owns any enabled mutable instrument.
    /// Session-only inventory must not affect linear Arrangement range policy.
    pub fn has_arrangement_mutable_instruments(&self) -> bool {
        self.arrangement
            .tracks()
            .iter()
            .any(crate::Track::has_mutable_instrument)
    }

    /// Conservative, cursor-independent guard for live structural section edits
    /// and history. Existing mutable MIDI identities cannot be relocated or
    /// expose previously unplayed onsets without a separate history proof.
    /// Length-only retained prefixes remain eligible when no onset is exposed.
    /// Missing ordinary source evidence requires stopped preparation. This is
    /// a read-only control query, not runtime handoff admission or DSP custody.
    /// The section coordinator separately validates clock and transport fences.
    pub fn section_midi_relocation_requires_stop(&self, candidate: &Self) -> bool {
        if !self.has_arrangement_mutable_instruments()
            && !candidate.has_arrangement_mutable_instruments()
        {
            return false;
        }
        if self.device_resource_scope.is_none() || candidate.device_resource_scope.is_none() {
            return true;
        }
        for compiled in self
            .arrangement
            .tracks()
            .iter()
            .chain(candidate.arrangement.tracks())
        {
            if !compiled.has_mutable_instrument() {
                continue;
            }
            let Some(id) = compiled.canonical_track_id() else {
                return true;
            };
            let (Some(old), Some(new)) = (
                self.mutable_track_revision(id),
                candidate.mutable_track_revision(id),
            ) else {
                return true;
            };
            if old.midi_source_track != new.midi_source_track {
                return true;
            }
            for old_clip in &old.arrangement_midi_sources {
                let Some(new_clip) = new
                    .arrangement_midi_sources
                    .binary_search_by_key(&old_clip.id, |clip| clip.id)
                    .ok()
                    .and_then(|index| new.arrangement_midi_sources.get(index))
                else {
                    continue;
                };
                if old_clip.start != new_clip.start
                    || old_clip.playback_offset != new_clip.playback_offset
                    || old_clip.source != new_clip.source
                    || old_clip.content_view != new_clip.content_view
                {
                    return true;
                }
                if new_clip.length > old_clip.length {
                    let Some(track) = candidate
                        .arrangement
                        .tracks()
                        .iter()
                        .find(|track| track.canonical_track_id() == Some(id))
                    else {
                        return true;
                    };
                    let mut exposed = false;
                    if track
                        .for_each_mutable_midi_span(
                            old_clip.end_frame,
                            new_clip.end_frame,
                            |span| {
                                exposed |=
                                    span.clip == old_clip.id && span.start >= old_clip.end_frame;
                            },
                        )
                        .is_err()
                        || exposed
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Whether an Arrangement seek must reconstruct stateful history instead
    /// of using the direct absolute-frame sampler path. Built-in Sine and
    /// Operator MIDI notes are deterministic functions of note age and gate
    /// duration, so they can be evaluated directly at a seek destination.
    /// Mutable Arrangement instruments and stateful track/return processors
    /// carry history and require preroll. Return processors are checked
    /// conservatively even when no current send reaches them.
    pub fn arrangement_seek_requires_preroll(&self) -> bool {
        !self.arrangement.main_device_chain.is_empty()
            || self
                .arrangement
                .tracks()
                .iter()
                .any(|track| track.has_mutable_instrument() || track.has_device_chain())
            || self.arrangement.return_buses().iter().any(|bus| {
                bus.device_chain
                    .iter()
                    .any(|device| device.definition.is_some())
            })
    }

    /// A private finite renderer must replay the note history of foreign
    /// instruments before a nonzero bounce start.
    pub fn arrangement_bounce_requires_preroll(&self) -> bool {
        self.arrangement_seek_requires_preroll()
            || self.arrangement.tracks().iter().any(|track| {
                track
                    .foreign_instrument
                    .as_ref()
                    .is_some_and(|node| !node.bypassed)
            })
    }

    /// Source identity/configuration is independent of receiver position and
    /// every contributor's local effects. Reuse existing exact canonical device
    /// descriptors; this query creates no competing source/resource owner.
    pub(crate) fn analytic_context_for_track(&self, track: TrackId) -> Option<u16> {
        self.arrangement()
            .tracks()
            .iter()
            .position(|row| row.canonical_track_id() == Some(track))
            .and_then(|index| self.source_routing.source(index))
            .and_then(|source| self.arrangement.shared_contexts().index_for_source(source))
            .or_else(|| self.arrangement.shared_contexts().index_for_track(track))
    }

    pub(crate) fn track_retains_source_configuration(
        &self,
        candidate: &Self,
        track: TrackId,
    ) -> bool {
        let source = |revision: &Self| {
            revision
                .arrangement()
                .tracks()
                .iter()
                .position(|row| row.canonical_track_id() == Some(track))
                .filter(|index| revision.source_routing.receiver_index(*index).is_some())
                .and_then(|index| revision.source_routing.source(index))
        };
        source(self)
            .zip(source(candidate))
            .is_some_and(|(old, new)| {
                old == new && self.retained_source_configuration_matches(candidate, old)
            })
    }

    pub(crate) fn retained_source_identity_matches(
        &self,
        candidate: &Self,
        source: hex_project::SourceId,
    ) -> bool {
        let device = |revision: &Self| {
            revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .find(|(index, track)| {
                    revision.source_routing.source(*index) == Some(source)
                        && track.receives_source_audio()
                })
                .and_then(|(_, track)| track.canonical_track_id())
                .and_then(|track| revision.mutable_track_revision(track))
                .and_then(|track| track.instrument.as_ref())
                .map(|device| (device.id, device.state.type_id(), device.bypassed))
        };
        // Retargeting may change tuning/state only through the target-bound
        // provider proof. Exact configuration matching below still includes
        // tuning; source identity alone never grants runtime compatibility.
        device(self)
            .zip(device(candidate))
            .is_some_and(|(old, new)| old == new)
    }

    pub(crate) fn retained_source_configuration_matches(
        &self,
        candidate: &Self,
        source: hex_project::SourceId,
    ) -> bool {
        fn instrument(
            revision: &PreparedRevision,
            source: hex_project::SourceId,
        ) -> Option<&PreparedDeviceIdentity> {
            revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .find(|(index, track)| {
                    revision.source_routing.source(*index) == Some(source)
                        && track.receives_source_audio()
                })
                .and_then(|(_, track)| track.canonical_track_id())
                .and_then(|track| revision.mutable_track_revision(track))
                .and_then(|track| track.instrument.as_ref())
        }
        self.arrangement.shared_contexts().tuning_identity()
            == candidate.arrangement.shared_contexts().tuning_identity()
            && instrument(self, source)
                .zip(instrument(candidate, source))
                .is_some_and(|(old, new)| old == new)
    }

    /// Conservative callback-side source proof with bounded state comparison.
    pub(crate) fn retained_source_configuration_matches_on_callback(
        &self,
        candidate: &Self,
        source: hex_project::SourceId,
    ) -> bool {
        fn instrument(
            revision: &PreparedRevision,
            source: hex_project::SourceId,
        ) -> Option<&PreparedDeviceIdentity> {
            revision
                .arrangement()
                .tracks()
                .iter()
                .enumerate()
                .find(|(index, _)| revision.source_routing.source(*index) == Some(source))
                .and_then(|(_, track)| track.canonical_track_id())
                .and_then(|track| revision.mutable_track_revision(track))
                .and_then(|track| track.instrument.as_ref())
        }
        self.arrangement.shared_contexts().tuning_identity()
            == candidate.arrangement.shared_contexts().tuning_identity()
            && instrument(self, source)
                .zip(instrument(candidate, source))
                .is_some_and(|(old, new)| {
                    old.id == new.id
                        && old.parent == new.parent
                        && old.bypassed == new.bypassed
                        && old.state.processing_eq_on_callback(&new.state)
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

    /// `mutable_arrangement_track_matches` across an edited base tempo map:
    /// the same topology and beat-anchored sources, whose frames may move.
    pub(crate) fn mutable_arrangement_track_matches_retimed(
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
        self.arrangement.shared_contexts().tuning_identity()
            == other.arrangement.shared_contexts().tuning_identity()
            && old.topology_matches(new)
            && old.arrangement_sources_match_retimed(new)
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

    /// Preallocation quote for ordinary compilation's complete predecessor evidence. Counts
    /// canonical source/state backing conservatively, including shared duplicates. One additional
    /// same-project descriptor set is reserved for tuning-continuation validation scratch; this
    /// construction-only allowance does not multiply retained inventory. Native source program
    /// construction is separately quoted once, including retained/temporary overlap. Actual Vec
    /// capacities are checked after construction. Legacy closed compilation is unchanged.
    pub fn device_handoff_metadata_construction_bytes(project: &Project) -> Result<usize, Invalid> {
        Self::device_handoff_metadata_construction_bytes_with_input(project, None)
    }

    /// Ordinary construction with one optional source-owned automation target.
    pub fn device_handoff_metadata_construction_bytes_with_input(
        project: &Project,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<usize, Invalid> {
        let bytes = (|| {
            let mut bytes = project
                .tracks()
                .len()
                .checked_mul(std::mem::size_of::<PreparedMutableTrackRevision>())?;
            for track in project.tracks() {
                bytes = bytes.checked_add(
                    track
                        .devices()
                        .len()
                        .checked_mul(std::mem::size_of::<PreparedDeviceIdentity>())?,
                )?;
                for device in track.instrument().into_iter().chain(track.devices()) {
                    bytes = bytes.checked_add(device.state().retained_control_metadata_bytes()?)?;
                }
                let source = project.track(project.midi_source_track(track.id()))?;
                bytes = bytes.checked_add(
                    source
                        .midi_clips()
                        .len()
                        .checked_mul(std::mem::size_of::<PreparedArrangementMidiSource>())?,
                )?;
                for clip in source.midi_clips() {
                    bytes = bytes
                        .checked_add(clip.source_identity().retained_control_metadata_bytes()?)?;
                }
            }
            bytes
                .checked_add(authored_midi::construction_bytes(project).ok()?)?
                .checked_add(crate::PreparedSourceRouting::construction_bytes(project))?
                .checked_add(crate::source_effect_stage::metadata::construction_bytes(
                    project,
                    native_input,
                )?)
        })();
        let programs = crate::foreign_automation::candidate::quote_instruments(project)
            .map_err(|_| Invalid("vst3 instrument automation reservation overflow"))?;
        let candidate = bytes
            .and_then(|bytes| bytes.checked_add(programs))
            .ok_or(Invalid("device handoff metadata byte count overflow"))?;
        // Tuning continuation validates same_except_tuning before rebuilding predecessor
        // descriptors. It may coexist with the candidate; reserve that finite worst case even
        // when this generic preparation call does not know whether validation will be requested.
        let predecessor_validation_scratch = candidate;
        let native_programs = PreparedSessionInventory::instrument_automation_construction_bytes(
            project,
            native_input,
        )?;
        candidate
            .checked_add(predecessor_validation_scratch)
            .and_then(|bytes| bytes.checked_add(native_programs))
            .ok_or(Invalid("device handoff construction byte count overflow"))
    }

    /// Actual ordinary predecessor-evidence capacity. This conservative control-metadata
    /// allowance is retained per immutable graph until control destroys that graph, independently
    /// of migrating source leases. It is not a closed constructor or shared-allocation proof.
    pub(crate) fn device_handoff_metadata_bytes(&self) -> Result<usize, crate::AudioStorageError> {
        if self.device_resource_scope.is_none() {
            return Ok(0);
        }
        let bytes = (|| {
            let mut bytes = self
                .mutable_track_revisions
                .capacity()
                .checked_mul(std::mem::size_of::<PreparedMutableTrackRevision>())?;
            for track in &self.mutable_track_revisions {
                bytes = bytes.checked_add(
                    track
                        .effects
                        .capacity()
                        .checked_mul(std::mem::size_of::<PreparedDeviceIdentity>())?,
                )?;
                bytes = bytes.checked_add(
                    track
                        .arrangement_midi_sources
                        .capacity()
                        .checked_mul(std::mem::size_of::<PreparedArrangementMidiSource>())?,
                )?;
                for device in track.instrument.iter().chain(&track.effects) {
                    bytes = bytes.checked_add(device.state.retained_control_metadata_bytes()?)?;
                }
                for clip in &track.arrangement_midi_sources {
                    bytes = bytes.checked_add(clip.source.retained_control_metadata_bytes()?)?;
                }
            }
            for track in &self.arrangement().tracks {
                if let Some(program) = track
                    .foreign_instrument
                    .as_ref()
                    .and_then(|instrument| instrument.automation.foreign())
                {
                    bytes = bytes.checked_add(program.storage_bytes())?;
                }
            }
            bytes
                .checked_add(authored_midi::retained_bytes(&self.authored_midi_sources)?)?
                .checked_add(self.source_routing.retained_bytes())?
                .checked_add(crate::source_effect_stage::metadata::retained_bytes(
                    &self.arrangement().source_effects,
                )?)
        })();
        bytes.ok_or(crate::AudioStorageError::Overflow)
    }

    /// An authored slot discontinuity permits suppression of pre-cut gates, never chasing them.
    /// Both graphs need ordinary complete evidence; absent legacy evidence grants no exception.
    pub(crate) fn fresh_mutable_device_install(&self, candidate: &Self, track: TrackId) -> bool {
        if self.device_resource_scope.is_none()
            || candidate.device_resource_scope.is_none()
            || self.arrangement.shared_contexts().tuning_identity()
                != candidate.arrangement.shared_contexts().tuning_identity()
        {
            return false;
        }
        let Some(new) = candidate.mutable_track_revision(track) else {
            return false;
        };
        let Some(new_device) = new.instrument.as_ref().filter(|device| !device.bypassed) else {
            return false;
        };
        let Some(old) = self.mutable_track_revision(track) else {
            // Complete ordinary evidence distinguishes a genuinely new canonical track.
            return !self
                .arrangement
                .tracks()
                .iter()
                .any(|old| old.canonical_track_id() == Some(track));
        };
        old.instrument.as_ref().is_none_or(|device| {
            device.bypassed || device.id != new_device.id || device.state != new_device.state
        })
    }

    /// Missing DSP history is suppressed for a genuinely added/restored or newly enabled
    /// authored note. Complete evidence prevents a moved clip or an old note outside playback
    /// markers from being mistaken for a new note. This grants no voice/history continuity.
    pub(crate) fn newly_authored_mutable_note(
        &self,
        candidate: &Self,
        key: crate::mutable_instrument::MutableOccurrenceKey,
        session: bool,
    ) -> bool {
        if self.device_resource_scope.is_none()
            || candidate.device_resource_scope.is_none()
            || !self.mutable_track_topology_matches(candidate, key.track)
        {
            return false;
        }
        let source = |revision: &Self| {
            revision
                .mutable_track_revision(key.track)
                .map(|track| track.midi_source_track)
        };
        let (Some(old_root), Some(new_root)) = (source(self), source(candidate)) else {
            return false;
        };
        old_root == new_root
            && authored_midi::newly_authored(
                &self.authored_midi_sources,
                &candidate.authored_midi_sources,
                key.clip,
                key.note,
                new_root,
                session,
            )
    }

    /// Exact authored inactivity for an existing prepared MIDI instance. Missing
    /// or deleted clips retain their separate handoff policy. Routed sources
    /// are queried through their destination's canonical descriptor.
    pub(crate) fn midi_clip_inactive(&self, track: TrackId, clip: hex_project::ClipId) -> bool {
        if let Some(clip) = self.session.midi_clip_for_track(clip, track) {
            return !clip.active;
        }
        self.mutable_track_revision(track)
            .and_then(|track| {
                track
                    .arrangement_midi_sources
                    .binary_search_by_key(&clip, |source| source.id)
                    .ok()
                    .and_then(|index| track.arrangement_midi_sources.get(index))
            })
            .is_some_and(|clip| !clip.active)
    }

    pub(crate) fn midi_note_inactive(
        &self,
        clip: hex_project::ClipId,
        note: hex_project::NoteId,
    ) -> bool {
        authored_midi::inactive_note(&self.authored_midi_sources, clip, note)
    }

    /// A retained prefix is a strict narrowing of the same source clock, not
    /// an edited/replaced source. Handoff separately validates clock/loop and
    /// actual dispatched occurrence proofs before allowing a shorter gate.
    pub(crate) fn retained_midi_prefix_end(
        &self,
        candidate: &Self,
        key: crate::mutable_instrument::MutableOccurrenceKey,
    ) -> Option<(hex_project::BeatTime, u64)> {
        let old = self.mutable_track_revision(key.track)?;
        let new = candidate.mutable_track_revision(key.track)?;
        if !old.topology_matches(new) || old.midi_source_track != new.midi_source_track {
            return None;
        }
        fn find(
            track: &PreparedMutableTrackRevision,
            clip: hex_project::ClipId,
        ) -> Option<&PreparedArrangementMidiSource> {
            track
                .arrangement_midi_sources
                .binary_search_by_key(&clip, |source| source.id)
                .ok()
                .and_then(|index| track.arrangement_midi_sources.get(index))
        }
        let old = find(old, key.clip)?;
        let new = find(new, key.clip)?;
        (old.active
            && new.active
            && old.content_view == new.content_view
            && old.source == new.source
            && old.start == new.start
            && old.playback_offset == new.playback_offset
            && new.length < old.length)
            .then(|| {
                new.start
                    .raw()
                    .checked_add(new.length.raw())
                    .map(|end| (hex_project::BeatTime::from_raw(end), new.end_frame))
            })
            .flatten()
    }

    pub(crate) fn mutable_arrangement_sources_match(
        &self,
        candidate: &Self,
        track: TrackId,
    ) -> bool {
        let source_root = |revision: &Self| {
            revision
                .arrangement
                .tracks()
                .iter()
                .enumerate()
                .find(|(_, value)| value.canonical_track_id() == Some(track))
                .and_then(|(index, value)| {
                    revision
                        .arrangement
                        .tracks()
                        .get(value.midi_source_index.unwrap_or(index))
                })
                .and_then(crate::Track::canonical_track_id)
        };
        match (
            self.mutable_track_revision(track),
            candidate.mutable_track_revision(track),
        ) {
            (Some(old), Some(new)) => {
                source_root(self) == source_root(candidate) && old.arrangement_sources_match(new)
            }
            (None, Some(_)) => !self
                .arrangement
                .tracks()
                .iter()
                .any(|old| old.canonical_track_id() == Some(track)),
            _ => false,
        }
    }

    /// Callback-bounded equality of immutable Arrangement source identities.
    /// Source identity uses copy-on-write pointer equality, not note traversal.
    pub(crate) fn bounded_mutable_arrangement_sources_match(
        &self,
        candidate: &Self,
        track: TrackId,
        maximum: usize,
    ) -> bool {
        self.source_track_revision(track)
            .zip(candidate.source_track_revision(track))
            .is_some_and(|(old, new)| {
                old.midi_source_track == new.midi_source_track
                    && old.arrangement_midi_sources.len() <= maximum
                    && new.arrangement_midi_sources.len() <= maximum
                    && old.arrangement_sources_match(new)
            })
    }

    #[cfg(test)]
    pub(crate) fn has_mutable_track_revision(&self, track: TrackId) -> bool {
        self.mutable_track_revision(track).is_some()
    }

    fn mutable_track_revision(&self, track: TrackId) -> Option<&PreparedMutableTrackRevision> {
        self.source_track_revision(track)
            .filter(|revision| !revision.continuity_only)
    }

    #[cfg(test)]
    pub(crate) fn has_source_track_revision(&self, track: TrackId) -> bool {
        self.source_track_revision(track).is_some()
    }

    fn source_track_revision(&self, track: TrackId) -> Option<&PreparedMutableTrackRevision> {
        self.mutable_track_revisions
            .binary_search_by_key(&track, |revision| revision.id)
            .ok()
            .and_then(|index| self.mutable_track_revisions.get(index))
    }
}

fn prepare_mutable_track_revisions(
    project: &Project,
    arrangement: &PlaybackProject,
    complete_source_evidence: bool,
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
    // Keep bounded source/topology evidence for every contributor to a mutable
    // source as well as for providers that authorize a future analytic import.
    // Only the receiver owns the mutable runtime; contributor identity still
    // matters when a live handoff proves that its notes did not change.
    let owns_descriptor = |track: &crate::Track| {
        complete_source_evidence
            || track.has_mutable_instrument()
            || track
                .instrument_id
                .and_then(|device| arrangement.shared_contexts().owner_for_device(device))
                .is_some_and(|owner| owner.context().import_identity().is_some())
    };
    let needs_descriptor = |track: &crate::Track| {
        owns_descriptor(track)
            || track
                .source_receiver_index()
                .and_then(|index| arrangement.tracks().get(index))
                .is_some_and(|receiver| {
                    receiver.source_id() == track.source_id() && receiver.has_mutable_instrument()
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
        for clip in source.midi_clips() {
            let end = clip
                .start()
                .raw()
                .checked_add(clip.length().raw())
                .ok_or(Invalid("midi descriptor end overflow"))?;
            arrangement_midi_sources.push(PreparedArrangementMidiSource {
                content_view: clip.content_view(),
                active: clip.active(),
                playback_offset: clip.playback_offset(),
                id: clip.id(),
                start: clip.start(),
                length: clip.length(),
                end_frame: project
                    .tempo_map()
                    .beat_to_frame(hex_project::BeatTime::from_raw(end))
                    .map_err(|error| Invalid(error.0))?,
                source: clip.source_identity(),
            });
        }
        arrangement_midi_sources.sort_unstable_by_key(|clip| clip.id);

        revisions.push(PreparedMutableTrackRevision {
            id: track.id(),
            continuity_only: !owns_descriptor(compiled),
            midi_source_track: source_id,
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
                                && a.automation == b.automation
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

#[cfg(test)]
mod ordinary_streaming_tests;

#[cfg(test)]
#[path = "prepared_revision/serial_unit_tests.rs"]
mod serial_unit_tests;
