//! Preparation-only Arrangement and opt-in sealed Session/revision assembly.
//! Not a storage-complete graph, publication admission or lifetime authority.
//! The audio host must retain this custody through cancellation and abandonment.
use crate::audio_only_inputs::*;
mod arrangement_clips;
pub(crate) use arrangement_clips::sampler_workspace_bound;
mod storage;
pub(crate) mod transformed;
pub(crate) use arrangement_clips::ArrangementClips;
pub(crate) use transformed::transformed_audio_quote;
#[cfg(test)]
#[path = "session_inventory_assembly_tests.rs"]
mod assembly_tests;
#[cfg(test)]
#[path = "session_audio_preparation_tests.rs"]
mod session_source_tests;
#[cfg(test)]
#[path = "session_shared_preparation_tests.rs"]
mod shared_tests;
#[cfg(test)]
#[path = "session_structure_preparation_tests.rs"]
mod structure_tests;
#[cfg(test)]
#[path = "session_tempo_preparation_tests.rs"]
mod tempo_tests;
pub use arrangement_clips::{
    ArrangementClipCapacity, ArrangementRootCapacity,
    returns::{ArrangementReturnCapacity, ArrangementReturnMetadata, ReturnFeedCapacity},
    tracks::{ArrangementTrackCapacity, ArrangementTrackMetadata},
};
#[cfg(test)]
mod arrangement_clip_tests;
use std::{
    any::Any,
    mem,
    panic::{AssertUnwindSafe, catch_unwind},
};

pub(crate) fn validate_constructor_project(
    project: &hex_project::Project,
) -> Result<(), AudioStorageError> {
    validate_constructor_project_with_effects(project, None)
}
pub(crate) fn validate_constructor_project_with_effects(
    project: &hex_project::Project,
    effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
) -> Result<(), AudioStorageError> {
    crate::device_modulation::validate_source_owners(project)
        .map_err(|_| AudioStorageError::UnsupportedGraph)?;
    // Bound suffixes use the ordinary receiver publication, whose recipe owns
    // the source program. The sealed root does not retain these recipes.
    if project.native_sources().iter().any(|source| {
        source.receiver().is_some()
            && source
                .post_instrument_effects()
                .iter()
                .any(|effect| !effect.modulation().is_empty())
    }) {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    let supported = |d: &hex_project::Device| {
        d.state().asset_references().is_empty()
            && (d.state().type_id() == hex_devices::utility::UTILITY_TYPE_ID
                || effects.is_some_and(|r| r.definition(d.state().type_id()).is_some()))
    };
    if effects.is_some()
        && project
            .tracks()
            .iter()
            .flat_map(|t| t.devices())
            .chain(project.return_tracks().iter().flat_map(|r| r.devices()))
            .chain(project.main_devices())
            .any(|d| !supported(d))
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    let sampler = if effects.is_some() {
        crate::native_device_resources::current_sampler::validate_project_with_effects(
            project, true,
        )?
    } else {
        crate::native_device_resources::current_sampler::validate_project(project)?
    };
    if sampler {
        crate::gain_envelope::validate_project(project)
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        return Ok(());
    }
    if project.tracks().iter().any(|t| {
        t.devices().iter().any(|d| !supported(d))
            || t.instrument().is_some()
            || !t.midi_clips().is_empty()
    }) || project
        .return_tracks()
        .iter()
        .any(|t| t.devices().iter().any(|d| !supported(d)))
        || project.main_devices().iter().any(|d| !supported(d))
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    if effects.is_none()
        && (!project.main_devices().is_empty()
            || project
                .return_tracks()
                .iter()
                .any(|r| !r.devices().is_empty()))
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    // Authored feeds use rooted graph custody. Stream demand independently
    // checks its supported source and routing class.
    if !project.midi_routes().is_empty()
        || project
            .tracks()
            .iter()
            .any(|track| !track.gain_lane().spans().is_empty())
    {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    crate::gain_envelope::validate_project(project)
        .map_err(|_| AudioStorageError::UnsupportedGraph)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioPreparationPhase {
    Vacant,
    Constructing,
    ArrangementSources,
    ArrangementAndSessionSources,
    Cleaning,
    Recoverable,
    FailedIncomplete,
}
/// Nonowning, sealed root/incarnation identity. It cannot read or reclaim owners
/// without the matching live custodian. No Clone or public scalar constructor.
pub struct AudioPreparationToken {
    root: u64,
    incarnation: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Accepted-stage declarations/actual backing observations, not whole graph or
/// process memory. Refusals may contain only partially collected observations;
/// aggregate fields are authoritative only after successful full preflight.
pub struct AudioArrangementCapacity {
    pub source_slot_capacity: usize,
    pub source_slot_bytes: usize,
    pub previous_slot_backing_bytes: usize,
    pub custodian_inline_bytes: usize,
    pub input_container_bytes: usize,
    pub declared_source_bytes: usize,
    pub retained_stage_bytes: usize,
    pub construction_peak_bytes: usize,
    pub session: crate::SessionSourceCapacity,
    pub structure: crate::SessionStructureCapacity,
    pub tempo: crate::SessionTempoCapacity,
    pub shared: crate::SessionSharedCapacity,
    pub assembly: crate::SessionAssemblyCapacity,
    pub arrangement_clips: ArrangementClipCapacity,
    pub revision_roots: crate::AudioRevisionRootCapacity,
    pub native_source_metadata_bytes: usize,
    pub native_compiler_workspace_bytes: usize,
}
pub struct AudioArrangementRefusal {
    pub reason: AudioStorageError,
    pub inputs: OwnedAudioInputs,
    pub capacity: AudioArrangementCapacity,
}
/// A nonowning refusal: the caller's in-place bank remains in its original
/// persistent destination, including when partially reserved preparation fails.
#[derive(Debug)]
pub struct AudioArrangementStartRefusal {
    pub reason: AudioStorageError,
    pub capacity: AudioArrangementCapacity,
}
/// Borrowed compiler selections, not source custody or publication authority.
#[derive(Clone, Copy, Default)]
struct ConstructorResources<'a> {
    sampler: Option<(u64, &'a std::sync::atomic::AtomicBool)>,
    effects: Option<&'a hex_lattice::NativeDeviceRegistry<'a>>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
}
struct ReservedStages {
    slots: Vec<SourceSlot>,
    capacity: AudioArrangementCapacity,
    next: u64,
    signal_graph: bool,
}
#[derive(Clone, Copy, Default)]
struct Stages {
    include_session: bool,
    assemble_session: bool,
    assemble_arrangement: bool,
    assemble_tracks: bool,
    assemble_returns: bool,
    assemble_preview: bool,
    assemble_project: bool,
    assemble_revision: bool,
}
/// Canonical source correlation captured with the actual control-side project.
/// These numbers are not ownership or publication authority. Source-catalog
/// generation is a separate identity and may differ from document generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSourceContext {
    pub project_generation: u64,
    pub editor_revision: u64,
}
/// Borrowed canonical metadata plus consumed bank; no audio/stream dependency
/// enters the engine. A host need not depend on hex-project to accept this input.
pub struct AudioArrangementInput<'a> {
    project: &'a hex_project::Project,
    generation: u64,
    inputs: OwnedAudioInputs,
    stages: Stages,
    source_context: Option<AudioSourceContext>,
    effects: Option<&'a hex_lattice::NativeDeviceRegistry<'a>>,
    native_input: Option<crate::device_automation::NativeParameterInputTarget>,
}
impl<'a> AudioArrangementInput<'a> {
    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
    }
    pub fn new(
        project: &'a hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
    ) -> Self {
        Self {
            project,
            generation,
            inputs,
            stages: Stages::default(),
            source_context: None,
            effects: None,
            native_input: None,
        }
    }
    /// Borrow foreign effect facts for the same closed constructor. No provider
    /// owner survives preparation and no native source/publication is admitted.
    pub fn with_effect_registry(
        mut self,
        effects: &'a hex_lattice::NativeDeviceRegistry<'a>,
    ) -> Self {
        self.effects = Some(effects);
        self
    }
    /// Select one already validated native input for this exact canonical project.
    /// The constructor revalidates it before reservation or source-bank consumption.
    pub fn with_native_parameter_input(
        mut self,
        input: crate::device_automation::NativeParameterInputTarget,
    ) -> Self {
        self.native_input = Some(input);
        self
    }
    /// Low-level correlation bridge. Prefer a Session-owned immutable snapshot
    /// that derives the Project and these counters together; this grants no
    /// constructor, source lease, readiness or native ownership authority.
    pub fn with_source_context(mut self, context: AudioSourceContext) -> Self {
        self.source_context = Some(context);
        self
    }
    /// Explicitly add the new persistent Session audio source/name stage.
    /// Previous legacy inventories are never adopted; cache aliases originate
    /// only in this same input bank and remain in the same preparation custodian.
    pub fn with_session_sources(mut self) -> Self {
        self.stages.include_session = true;
        self
    }
    /// Explicit sealed final Session assembly. No legacy inventory/clip/Arc is
    /// returned; publication and complete accounting remain unavailable.
    pub fn with_session_inventory(mut self) -> Self {
        self.stages.include_session = true;
        self.stages.assemble_session = true;
        self
    }
    /// Explicit private Arrangement clip targets plus the sealed Session stage.
    /// No Track/Project/PreparedRevision or publication capability is exposed.
    pub fn with_audio_clip_inventory(mut self) -> Self {
        self.stages.include_session = true;
        self.stages.assemble_session = true;
        self.stages.assemble_arrangement = true;
        self
    }
    /// Add sealed private Track targets; no Project/PreparedRevision/publication.
    pub fn with_audio_track_inventory(self) -> Self {
        let mut input = self.with_audio_clip_inventory();
        input.stages.assemble_tracks = true;
        input
    }
    /// Add sealed Return buses/feed plan; no Project/root publication capability.
    pub fn with_audio_return_inventory(self) -> Self {
        let mut input = self.with_audio_track_inventory();
        input.stages.assemble_returns = true;
        input
    }
    /// Assemble an actual sealed empty MIDI preview for the audio-only class.
    /// Root/shared-graph assembly, complete accounting and admission stay gated.
    pub fn with_audio_preview_inventory(self) -> Self {
        let mut input = self.with_audio_return_inventory();
        input.stages.assemble_preview = true;
        input
    }
    /// Assemble a sealed private Arrangement root from the already reserved
    /// audio-only children. No Project/Arc/publication capability is exposed.
    pub fn with_audio_project_inventory(self) -> Self {
        let mut input = self.with_audio_preview_inventory();
        input.stages.assemble_project = true;
        input
    }
    /// Compose the actual private PreparedRevision from the already constructed
    /// audio-only Project, Session and preview. No owner/publication escape.
    pub fn with_audio_prepared_revision(self) -> Self {
        let mut input = self.with_audio_project_inventory();
        input.stages.assemble_revision = true;
        input
    }
    pub fn refuse(self, reason: AudioStorageError) -> Box<AudioArrangementRefusal> {
        Box::new(AudioArrangementRefusal {
            reason,
            inputs: self.inputs,
            capacity: AudioArrangementCapacity::default(),
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Disposal {
    #[default]
    Unstarted,
    Pending,
    Complete,
    Transferred,
}
// One exact handle location per Arrangement clip, not a content/key identity.
// Geometry is a compiler intermediate; no source owner escapes through views.
struct SourceSlot {
    active: bool,
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    source: usize,
    handle: u64,
    start: u64,
    end: u64,
    source_offset: usize,
    source_length: usize,
    phase: u64,
    gain: f32,
    looping: bool,
    fade: hex_project::ClipFade,
    owner: Option<OwnedAudioSource>,
    // Fresh constructor root stays here after its distinct graph alias moves.
    prepared_origin: Option<OwnedAudioSource>,
    sparse_frames: Option<usize>,
    sparse: Option<std::sync::Arc<crate::SparseFrameMap>>,
    transformed: bool,
    disposal: Disposal,
    #[cfg(test)]
    probe: Option<DropProbe>,
}
impl SourceSlot {
    fn source_owner<'a>(
        &'a self,
        bank: &'a OwnedAudioInputs,
    ) -> Result<&'a OwnedAudioSource, AudioStorageError> {
        match (self.transformed, self.prepared_origin.as_ref()) {
            (true, Some(origin)) if origin.handle != self.handle => Ok(origin),
            (false, None) => bank
                .sources
                .get(self.source)
                .map(|(_, source)| source)
                .ok_or(AudioStorageError::SourceMismatch),
            _ => Err(AudioStorageError::SourceMismatch),
        }
    }
}
#[cfg(test)]
impl Drop for SourceSlot {
    fn drop(&mut self) {
        drop(self.probe.take());
    }
}
#[cfg(test)]
pub(super) struct DropProbe(Option<Box<dyn FnOnce() + Send>>);
#[cfg(test)]
impl Drop for DropProbe {
    fn drop(&mut self) {
        // The actual owned slot field destructor is exercised by cleanup tests.
        if let Some(call) = self.0.take() {
            call();
        }
    }
}
/// Persistent owner storage. No Drop implementation performs cleanup or joins.
/// An incomplete custody must remain rooted in the existing lifecycle quarantine;
/// callers must not detach it to a UI handle. No public raw owner getter exists.
pub struct AudioPreparationCustody {
    root: u64,
    source_context: Option<AudioSourceContext>,
    incarnation: u64,
    phase: AudioPreparationPhase,
    bank: Option<OwnedAudioInputs>,
    current_sampler: crate::native_device_resources::current_sampler::CurrentSamplerConstruction,
    slots: Vec<SourceSlot>,
    next_disposal: usize,
    first_panic: Option<Box<dyn Any + Send>>,
    capacity: AudioArrangementCapacity,
    reason: Option<AudioStorageError>,
    session: crate::session_audio_preparation::SessionSourceConstruction,
    structure: crate::session_structure_preparation::SessionStructureConstruction,
    tempo: crate::session_tempo_preparation::SessionTempoConstruction,
    shared: crate::session_shared_preparation::SessionSharedConstruction,
    assembly: crate::session_inventory::audio_assembly::SessionAssembly,
    arrangement_clips: arrangement_clips::ArrangementClips,
    revision: crate::prepared_revision::assembly::RevisionAssembly,
    #[cfg(test)]
    fail_after: Option<usize>,
    #[cfg(test)]
    panic_after: Option<usize>,
}
impl AudioPreparationCustody {
    pub(crate) fn native_sampler_pcm_capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.resident_pcm_capacity_frames()
    }
    pub(crate) fn resident_pcm_capacity_frames(&self) -> Result<u64, AudioStorageError> {
        let bank = self
            .current_sampler
            .bank()
            .or(self.bank.as_ref())
            .ok_or(AudioStorageError::SourceMismatch)?;
        let bank_frames =
            bank.sources
                .iter()
                .enumerate()
                .try_fold(0u64, |n, (i, (_, source))| {
                    let SourceKind::Resident(audio) = &source.kind else {
                        return Ok(n);
                    };
                    if bank.sources[..i]
                        .iter()
                        .any(|(_, old)| crate::audio_input_storage::same(old, source))
                    {
                        return Ok(n);
                    }
                    n.checked_add(
                        u64::try_from(audio.pcm_capacity_frames())
                            .map_err(|_| AudioStorageError::Overflow)?,
                    )
                    .ok_or(AudioStorageError::Overflow)
                })?;
        self.slots
            .iter()
            .enumerate()
            .try_fold(bank_frames, |n, (index, slot)| {
                let Some(origin) = slot.prepared_origin.as_ref() else {
                    return Ok(n);
                };
                let SourceKind::Resident(audio) = &origin.kind else {
                    return Err(AudioStorageError::SourceMismatch);
                };
                if !slot.transformed {
                    return Err(AudioStorageError::SourceMismatch);
                }
                if bank
                    .sources
                    .iter()
                    .any(|(_, old)| crate::audio_input_storage::same(old, origin))
                    || self.slots[..index]
                        .iter()
                        .filter_map(|slot| slot.prepared_origin.as_ref())
                        .any(|old| crate::audio_input_storage::same(old, origin))
                {
                    return Ok(n);
                }
                n.checked_add(
                    u64::try_from(audio.pcm_capacity_frames())
                        .map_err(|_| AudioStorageError::Overflow)?,
                )
                .ok_or(AudioStorageError::Overflow)
            })
    }
    pub(crate) fn native_sampler_source_capacity(&self) -> Option<usize> {
        self.current_sampler
            .bank()
            .map(|bank| bank.sources.capacity())
    }
    pub(crate) fn native_sampler_capture(
        &self,
    ) -> Result<
        Option<(
            crate::owned_shared::OwnedShared<
                crate::native_device_resources::current_sampler::CurrentSamplerSources,
            >,
            u64,
        )>,
        AudioStorageError,
    > {
        self.current_sampler.runtime_capture()
    }
    pub(crate) fn candidate_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.revision.candidate_sources()
    }
    pub(crate) fn candidate_root(
        &self,
        token: &AudioPreparationToken,
    ) -> Result<(u64, u64, std::sync::Arc<crate::PreparedRevision>), AudioStorageError> {
        if !self.matches(token) || self.phase != AudioPreparationPhase::ArrangementAndSessionSources
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok((self.root, self.incarnation, self.revision.candidate_root()?))
    }

    pub(crate) fn candidate_root_view(
        &self,
        token: &AudioPreparationToken,
    ) -> Result<&std::sync::Arc<crate::PreparedRevision>, AudioStorageError> {
        if !self.matches(token) || self.phase != AudioPreparationPhase::ArrangementAndSessionSources
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.revision.candidate_root_view()
    }
    pub fn begin_input_on_control(
        &mut self,
        input: AudioArrangementInput<'_>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
        let result = self.begin_stages_on_control(
            input.project,
            input.generation,
            input.inputs,
            input.stages,
            input.effects,
            input.native_input,
        );
        if result.is_ok() {
            self.source_context = input.source_context;
        }
        result
    }
    pub(crate) fn source_context(&self) -> Option<AudioSourceContext> {
        self.source_context
    }
    pub fn try_new_on_control() -> Result<Self, AudioStorageError> {
        Ok(Self {
            root: handle()?,
            source_context: None,
            incarnation: 0,
            phase: AudioPreparationPhase::Vacant,
            bank: None,
            current_sampler: Default::default(),
            slots: Vec::new(),
            next_disposal: 0,
            first_panic: None,
            capacity: AudioArrangementCapacity::default(),
            reason: None,
            session: crate::session_audio_preparation::SessionSourceConstruction::default(),
            structure: crate::session_structure_preparation::SessionStructureConstruction::default(
            ),
            tempo: crate::session_tempo_preparation::SessionTempoConstruction::default(),
            shared: crate::session_shared_preparation::SessionSharedConstruction::default(),
            assembly: crate::session_inventory::audio_assembly::SessionAssembly::default(),
            arrangement_clips: arrangement_clips::ArrangementClips::default(),
            revision: crate::prepared_revision::assembly::RevisionAssembly::default(),
            #[cfg(test)]
            fail_after: None,
            #[cfg(test)]
            panic_after: None,
        })
    }
    pub fn phase(&self) -> AudioPreparationPhase {
        self.phase
    }
    pub fn reason(&self) -> Option<AudioStorageError> {
        self.reason
    }
    pub fn capacity_report(&self) -> AudioArrangementCapacity {
        self.capacity
    }
    pub fn matches(&self, token: &AudioPreparationToken) -> bool {
        token.root == self.root
            && token.incarnation == self.incarnation
            && self.phase != AudioPreparationPhase::Vacant
    }
    /// Before bank consumption, draft and constructor owners stay in persistent
    /// custody. Every identity, geometry and storage refusal returns the EXACT
    /// original bank; incomplete draft cleanup remains retained separately.
    /// Fatal OOM/abort is outside the recoverable API contract.
    pub fn begin_arrangement_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
        self.begin_stages_on_control(project, generation, inputs, Stages::default(), None, None)
    }
    fn begin_stages_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
        stages: Stages,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
        match self.reserve_stages_with_resources(
            project,
            generation,
            &inputs,
            stages,
            ConstructorResources {
                sampler: None,
                effects,
                native_input,
            },
        ) {
            Ok(reserved) => Ok(self.construct_reserved_on_control(
                project,
                Some(inputs),
                stages,
                reserved,
                None,
            )),
            Err(refusal) => Err(Box::new(AudioArrangementRefusal {
                reason: refusal.reason,
                inputs,
                capacity: refusal.capacity,
            })),
        }
    }
    /// Build only the closed audio/Utility prepared class; registered effects
    /// use the explicit effect-aware constructor companion. The caller reserves a
    /// persistent bank destination before invoking this route; it remains there
    /// throughout fallible reservation. The final move into this custodian is
    /// nonfallible, followed by the existing persistent caught construction.
    /// Context is correlation metadata, not source or publication authority.
    pub fn begin_prepared_revision_in_place_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        self.begin_prepared_revision_with(project, generation, context, inputs, || {})
    }
    fn begin_prepared_revision_with(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
        after_reserve: impl FnOnce(),
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        self.begin_prepared_revision_with_resources(
            project,
            generation,
            context,
            inputs,
            ConstructorResources::default(),
            after_reserve,
        )
    }
    pub(crate) fn begin_current_sampler_in_place_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
        work: u64,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        self.begin_prepared_revision_with_resources(
            project,
            generation,
            context,
            inputs,
            ConstructorResources {
                sampler: Some((work, cancel)),
                effects: None,
                native_input: None,
            },
            || {},
        )
    }
    pub(crate) fn begin_effect_revision_in_place_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        self.begin_effect_revision_with_input_in_place_on_control(
            project, generation, context, inputs, sampler, effects, None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_effect_revision_with_input_in_place_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        self.begin_prepared_revision_with_resources(
            project,
            generation,
            context,
            inputs,
            ConstructorResources {
                sampler,
                effects: Some(effects),
                native_input,
            },
            || {},
        )
    }
    fn begin_prepared_revision_with_resources(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        context: AudioSourceContext,
        inputs: &mut Option<OwnedAudioInputs>,
        resources: ConstructorResources<'_>,
        after_reserve: impl FnOnce(),
    ) -> Result<AudioPreparationToken, Box<AudioArrangementStartRefusal>> {
        let stages = Stages {
            include_session: true,
            assemble_session: true,
            assemble_arrangement: true,
            assemble_tracks: true,
            assemble_returns: true,
            assemble_preview: true,
            assemble_project: true,
            assemble_revision: true,
        };
        let Some(bank) = inputs.as_ref() else {
            return Err(Box::new(AudioArrangementStartRefusal {
                reason: AudioStorageError::SourceMismatch,
                capacity: AudioArrangementCapacity::default(),
            }));
        };
        let reserved = match catch_unwind(AssertUnwindSafe(|| {
            let reserved =
                self.reserve_stages_with_resources(project, generation, bank, stages, resources)?;
            after_reserve();
            Ok::<_, Box<AudioArrangementStartRefusal>>(reserved)
        })) {
            Ok(result) => result?,
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = AudioPreparationPhase::FailedIncomplete;
                return Err(Box::new(AudioArrangementStartRefusal {
                    reason: AudioStorageError::UnknownStorage,
                    capacity: self.capacity,
                }));
            }
        };
        let token = self.construct_reserved_on_control(
            project,
            inputs.take(),
            stages,
            reserved,
            resources.sampler.map(|(_, cancel)| cancel),
        );
        self.source_context = Some(context);
        Ok(token)
    }
    fn reserve_stages_with_resources(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: &OwnedAudioInputs,
        stages: Stages,
        resources: ConstructorResources<'_>,
    ) -> Result<ReservedStages, Box<AudioArrangementStartRefusal>> {
        let ConstructorResources {
            sampler,
            effects,
            native_input,
        } = resources;
        let Stages {
            include_session,
            assemble_session,
            assemble_arrangement,
            assemble_tracks,
            assemble_returns,
            assemble_preview,
            assemble_project,
            assemble_revision,
        } = stages;
        let mut slots = Vec::new();
        let mut capacity = AudioArrangementCapacity::default();
        let foreign_bytes = match crate::foreign_automation::candidate::quote(project) {
            Ok((bytes, _)) => bytes,
            Err(reason) => return Err(Box::new(AudioArrangementStartRefusal { reason, capacity })),
        };
        if foreign_bytes != 0 && (!assemble_revision || effects.is_none()) {
            return Err(Box::new(AudioArrangementStartRefusal {
                reason: AudioStorageError::UnsupportedGraph,
                capacity,
            }));
        }
        let result = self.preflight(
            project,
            generation,
            inputs,
            &mut slots,
            &mut capacity,
            resources,
        );
        let next = match result {
            Ok(next) => next,
            Err(reason) => {
                return Err(Box::new(AudioArrangementStartRefusal { reason, capacity }));
            }
        };
        if capacity
            .retained_stage_bytes
            .checked_add(foreign_bytes)
            .is_none_or(|bytes| bytes > inputs.reservation.max_prepared_data_bytes)
            || capacity
                .construction_peak_bytes
                .checked_add(foreign_bytes)
                .is_none_or(|bytes| bytes > inputs.reservation.max_construction_data_bytes)
        {
            return Err(Box::new(AudioArrangementStartRefusal {
                reason: AudioStorageError::ReservationExceeded,
                capacity,
            }));
        }
        if native_input.is_some() && !assemble_revision {
            return Err(Box::new(AudioArrangementStartRefusal {
                reason: AudioStorageError::UnsupportedGraph,
                capacity,
            }));
        }
        let transformed_workspace = capacity.native_compiler_workspace_bytes;
        self.source_context = None;
        if include_session {
            let previous_session_bytes = capacity.session.bytes().and_then(|n| {
                capacity
                    .structure
                    .bytes()
                    .and_then(|m| n.checked_add(m).ok_or(AudioStorageError::Overflow))
                    .and_then(|n| {
                        n.checked_add(capacity.tempo.bytes()?)
                            .and_then(|n| n.checked_add(capacity.shared.bytes().ok()?))
                            .and_then(|n| n.checked_add(capacity.assembly.bytes().ok()?))
                            .and_then(|n| {
                                n.checked_add(capacity.arrangement_clips.bytes().ok()?)
                                    .and_then(|n| n.checked_add(capacity.revision_roots.bytes()))
                            })
                            .ok_or(AudioStorageError::Overflow)
                    })
            });
            // Persistent destinations BEFORE fallible names/helper work. Bank
            // remains borrowed, so every pre-transfer refusal returns it intact.
            self.phase = AudioPreparationPhase::Constructing;
            self.structure.capacity.previous_growth_backing_bytes = 0;

            let result = catch_unwind(AssertUnwindSafe(|| {
                if let Some((work, cancel)) = sampler {
                    let allowance = inputs
                        .reservation
                        .max_prepared_data_bytes
                        .checked_sub(capacity.retained_stage_bytes)
                        .zip(
                            inputs
                                .reservation
                                .max_construction_data_bytes
                                .checked_sub(capacity.construction_peak_bytes),
                        )
                        .map(|(a, b)| a.min(b))
                        .ok_or(AudioStorageError::ReservationExceeded)?;
                    self.current_sampler.prepare_metadata(
                        inputs,
                        project,
                        work,
                        allowance,
                        cancel,
                        effects.is_some(),
                    )?;
                }

                if self.current_sampler.metadata().is_some() {
                    self.session.reserve_current_sampler(
                        project,
                        generation,
                        inputs,
                        slots.len(),
                        effects.is_some(),
                    )?;
                } else {
                    self.session
                        .reserve(project, generation, inputs, slots.len())?;
                }

                if self.current_sampler.metadata().is_some() {
                    self.structure
                        .reserve_current_sampler(project, &self.session)?;
                } else {
                    self.structure.reserve(project, &self.session)?;
                }

                self.tempo.reserve(project)?;
                if assemble_revision {
                    if let Some(metadata) = self.current_sampler.metadata() {
                        self.revision.reserve_with_sampler_and_effects(
                            project,
                            Some(metadata.id),
                            effects.is_some(),
                            native_input,
                        )?;
                    } else {
                        self.revision.reserve_with_sampler_and_effects(
                            project,
                            None,
                            effects.is_some(),
                            native_input,
                        )?;
                    }
                }
                let track_handles = if assemble_tracks {
                    project
                        .tracks()
                        .len()
                        .checked_mul(2)
                        .ok_or(AudioStorageError::Overflow)?
                } else {
                    0
                };
                let device_handles = if assemble_tracks {
                    project
                        .tracks()
                        .iter()
                        .map(|t| t.devices())
                        .chain(project.return_tracks().iter().map(|r| r.devices()))
                        .chain(std::iter::once(project.main_devices()))
                        .try_fold(0usize, |n, devices| {
                            n.checked_add(devices.len().checked_mul(4)?)?.checked_add(
                                devices
                                    .iter()
                                    .filter(|device| {
                                        crate::foreign_automation::candidate::has_authored(device)
                                    })
                                    .count(),
                            )
                        })
                        .ok_or(AudioStorageError::Overflow)?
                } else {
                    0
                };
                let return_handles = if assemble_returns {
                    project.return_tracks().len()
                } else {
                    0
                };
                let handles = inputs
                    .owning_handle_count()
                    .checked_add(slots.len())
                    .and_then(|n| n.checked_add(transformed::transformed_count(project)))
                    .and_then(|n| n.checked_add(self.session.capacity.reserved_handle_destinations))
                    .and_then(|n| n.checked_add(track_handles))
                    .and_then(|n| {
                        n.checked_add(
                            self.current_sampler
                                .metadata()
                                .map_or(0, |m| m.configurations().len() * 2 + 2),
                        )
                    })
                    .and_then(|n| {
                        n.checked_add(if self.current_sampler.metadata().is_some() {
                            project
                                .session_cells()
                                .iter()
                                .filter(|c| {
                                    matches!(c.content(), hex_project::SessionCellContent::Midi(_))
                                })
                                .count()
                                .checked_mul(4)?
                        } else {
                            0
                        })
                    })
                    .and_then(|n| n.checked_add(device_handles))
                    .and_then(|n| n.checked_add(return_handles))
                    .and_then(|n| n.checked_add(usize::from(assemble_preview)))
                    .and_then(|n| n.checked_add(if assemble_revision { 4 } else { 0 }))
                    .ok_or(AudioStorageError::Overflow)?;

                if let Some(metadata) = self.current_sampler.metadata() {
                    self.shared.reserve_current_sampler(
                        handles,
                        inputs.reservation.handle_obligations,
                        metadata.tuning,
                    )?;
                } else {
                    self.shared
                        .reserve(handles, inputs.reservation.handle_obligations)?;
                }
                if assemble_session {
                    self.assembly.reserve(
                        project,
                        &self.structure,
                        &self.session,
                        self.shared.inventory_handles(),
                    )?;

                    if let Some(metadata) = self.current_sampler.metadata() {
                        self.assembly.reserve_current_sampler_factories(
                            metadata,
                            self.shared.current_sampler_contexts()?,
                        )?;
                        self.assembly.reserve_current_sampler_midi_rows(project)?;
                    }
                }
                if assemble_arrangement {
                    let samplers = self
                        .current_sampler
                        .metadata()
                        .map(|metadata| {
                            self.shared
                                .current_sampler_contexts()
                                .map(|contexts| (metadata, contexts))
                        })
                        .transpose()?;

                    self.arrangement_clips.reserve_with_effects(
                        &slots,
                        inputs,
                        project,
                        stages,
                        samplers,
                        effects,
                        native_input,
                    )?;
                }
                Ok(())
            }));

            capacity.session = self.session.capacity;
            let structure_observation = self.structure.observe().map(|mut report| {
                report.previous_growth_backing_bytes =
                    self.structure.capacity.previous_growth_backing_bytes;
                report
            });
            if let Ok(report) = structure_observation {
                capacity.structure = report;
            }
            let tempo_observation = self.tempo.observe(true);
            if let Ok(report) = tempo_observation {
                capacity.tempo = report;
            }
            let shared_observation = self.assembly.shared_storage().and_then(|view| {
                self.shared
                    .observe_at(self.arrangement_clips.root_context(), view)
            });
            if let Ok(report) = shared_observation {
                capacity.shared = report;
            }
            let assembly_observation = self.assembly.capacity(assemble_session);
            if let Ok(report) = assembly_observation {
                capacity.assembly = report;
            }
            let arrangement_observation = self.arrangement_clips.capacity(assemble_arrangement);
            if let Ok(report) = arrangement_observation {
                capacity.arrangement_clips = report;
            }
            let revision_observation = self.revision.capacity();
            if let Ok(report) = revision_observation {
                capacity.revision_roots = report;
            }
            let result = match result {
                Ok(result) => result.and_then(|()| {
                    revision_observation?;
                    arrangement_observation?;
                    structure_observation?;
                    tempo_observation?;
                    shared_observation?;
                    assembly_observation?;
                    if assemble_project {
                        // Quote the actual selected Main chain BEFORE its birth.
                        capacity.arrangement_clips.root.main_prepared_bytes =
                            self.arrangement_clips.main_quote(project, native_input)?.0;
                    }
                    capacity.native_source_metadata_bytes = self.current_sampler.bytes()?;
                    let mut current = capacity
                        .session
                        .bytes()?
                        .checked_add(capacity.structure.bytes()?)
                        .and_then(|n| n.checked_add(capacity.native_source_metadata_bytes))
                        .and_then(|n| n.checked_add(capacity.tempo.bytes().ok()?))
                        .and_then(|n| n.checked_add(capacity.shared.bytes().ok()?))
                        .and_then(|n| n.checked_add(capacity.assembly.bytes().ok()?))
                        .and_then(|n| {
                            n.checked_add(capacity.arrangement_clips.bytes().ok()?)
                                .and_then(|n| n.checked_add(capacity.revision_roots.bytes()))
                        })
                        .ok_or(AudioStorageError::Overflow)?;
                    let mut compiler_reservation = None;
                    if let Some(metadata) = self.current_sampler.metadata() {
                        let (arrangement_midi, arrangement_scratch) =
                            arrangement_clips::sampler_compiler_quote(project)?;
                        let (session_midi, session_scratch) =
                            crate::session_inventory::audio_assembly::current_midi::compiler_quote(
                                project,
                            )?;
                        let final_midi = arrangement_midi
                            .checked_add(session_midi)
                            .ok_or(AudioStorageError::Overflow)?;
                        let scratch = arrangement_scratch
                            .checked_add(session_scratch)
                            .ok_or(AudioStorageError::Overflow)?;
                        let retained_base = capacity
                            .retained_stage_bytes
                            .checked_sub(previous_session_bytes?)
                            .and_then(|n| n.checked_add(current))
                            .ok_or(AudioStorageError::Overflow)?;
                        let growth = capacity
                            .session
                            .previous_backing_bytes
                            .checked_add(capacity.structure.previous_growth_backing_bytes)
                            .and_then(|n| {
                                n.checked_add(capacity.tempo.previous_growth_backing_bytes)
                            })
                            .and_then(|n| {
                                n.checked_add(capacity.assembly.previous_growth_backing_bytes)
                            })
                            .and_then(|n| {
                                n.checked_add(
                                    capacity.arrangement_clips.previous_growth_backing_bytes,
                                )
                            })
                            .ok_or(AudioStorageError::Overflow)?;
                        let peak_base = capacity
                            .construction_peak_bytes
                            .checked_sub(previous_session_bytes?)
                            .and_then(|n| n.checked_add(current))
                            .and_then(|n| n.checked_add(growth))
                            .ok_or(AudioStorageError::Overflow)?;
                        let retained_reservation = retained_base
                            .checked_add(final_midi)
                            .ok_or(AudioStorageError::Overflow)?;
                        let peak_reservation = peak_base
                            .checked_add(final_midi)
                            .and_then(|n| n.checked_add(scratch))
                            .ok_or(AudioStorageError::Overflow)?;
                        if retained_reservation > inputs.reservation.max_prepared_data_bytes
                            || peak_reservation > inputs.reservation.max_construction_data_bytes
                        {
                            return Err(AudioStorageError::ReservationExceeded);
                        }
                        compiler_reservation = Some((retained_reservation, peak_reservation));
                        // Every other fixed backing and all tracks' final MIDI
                        // overlap this conservative maximum scratch reservation.
                        // Check precedes the compiler, even for empty MIDI clips.
                        capacity.native_compiler_workspace_bytes = capacity
                            .native_compiler_workspace_bytes
                            .checked_add(scratch)
                            .ok_or(AudioStorageError::Overflow)?;
                        let old_assembly = capacity.assembly.bytes()?;
                        self.assembly.prepare_current_sampler_midi(
                            project,
                            metadata,
                            self.shared.current_sampler_contexts()?,
                        )?;
                        capacity.assembly = self.assembly.capacity(true)?;
                        current = current
                            .checked_sub(old_assembly)
                            .and_then(|n| n.checked_add(capacity.assembly.bytes().ok()?))
                            .ok_or(AudioStorageError::Overflow)?;
                        let old = capacity.arrangement_clips.bytes()?;
                        self.arrangement_clips.compile_current_sampler_tracks(
                            project,
                            metadata,
                            self.shared.current_sampler_contexts()?,
                        )?;
                        capacity.arrangement_clips = self.arrangement_clips.capacity(true)?;
                        if assemble_project {
                            capacity.arrangement_clips.root.main_prepared_bytes =
                                self.arrangement_clips.main_quote(project, native_input)?.0;
                        }
                        current = current
                            .checked_sub(old)
                            .and_then(|n| n.checked_add(capacity.arrangement_clips.bytes().ok()?))
                            .ok_or(AudioStorageError::Overflow)?;
                    }
                    capacity
                        .arrangement_clips
                        .root
                        .main_construction_overlap_bytes =
                        self.arrangement_clips.main_quote(project, native_input)?.1;
                    capacity.retained_stage_bytes = capacity
                        .retained_stage_bytes
                        .checked_sub(previous_session_bytes?)
                        .ok_or(AudioStorageError::Overflow)?
                        .checked_add(current)
                        .ok_or(AudioStorageError::Overflow)?;
                    capacity.construction_peak_bytes = capacity
                        .construction_peak_bytes
                        .checked_sub(previous_session_bytes?)
                        .ok_or(AudioStorageError::Overflow)?
                        .checked_add(current)
                        .and_then(|n| {
                            n.checked_add(
                                capacity
                                    .native_compiler_workspace_bytes
                                    .checked_sub(transformed_workspace)?,
                            )
                        })
                        .and_then(|n| n.checked_add(capacity.session.previous_backing_bytes))
                        .and_then(|n| {
                            n.checked_add(capacity.structure.previous_growth_backing_bytes)
                        })
                        .and_then(|n| n.checked_add(capacity.tempo.previous_growth_backing_bytes))
                        .and_then(|n| {
                            n.checked_add(capacity.assembly.previous_growth_backing_bytes)
                        })
                        .and_then(|n| {
                            n.checked_add(capacity.arrangement_clips.previous_growth_backing_bytes)
                        })
                        .and_then(|n| {
                            n.checked_add(
                                capacity
                                    .arrangement_clips
                                    .root
                                    .main_construction_overlap_bytes
                                    .checked_add(
                                        capacity
                                            .arrangement_clips
                                            .root
                                            .signal_graph_construction_overlap_bytes,
                                    )?,
                            )
                        })
                        .ok_or(AudioStorageError::Overflow)?;
                    // Keep the admitted conservative compiler reservation in
                    // the aggregate report even when actual vectors are smaller.
                    // Reusing the reported exact caps must still admit preflight.
                    if let Some((retained, peak)) = compiler_reservation {
                        capacity.retained_stage_bytes = capacity.retained_stage_bytes.max(retained);
                        capacity.construction_peak_bytes =
                            capacity.construction_peak_bytes.max(peak);
                    }
                    if capacity.retained_stage_bytes > inputs.reservation.max_prepared_data_bytes
                        || capacity.construction_peak_bytes
                            > inputs.reservation.max_construction_data_bytes
                    {
                        return Err(AudioStorageError::ReservationExceeded);
                    }
                    if assemble_project {
                        self.arrangement_clips.reserve_main_carrier(
                            project,
                            effects,
                            native_input,
                        )?;
                    }
                    Ok(())
                }),
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    Err(AudioStorageError::UnknownStorage)
                }
            };
            if let Err(reason) = result {
                if self.tempo.incomplete() {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                }
                if self.phase != AudioPreparationPhase::FailedIncomplete {
                    if self.dispose_arrangement_clips_on_control()
                        && self.arrangement_clips.clear_disposed().is_ok()
                        && self.dispose_assembly_on_control()
                        && self.assembly.clear_disposed().is_ok()
                        && self.dispose_session_on_control()
                        && self.dispose_structure_on_control()
                        && self.dispose_tempo_on_control()
                        && self.dispose_shared_on_control()
                        && self.shared.clear_disposed().is_ok()
                        && self.session.clear_disposed_rows().is_ok()
                        && self.structure.clear_disposed_rows().is_ok()
                        && self.current_sampler.clear_metadata().is_ok()
                    {
                        self.phase = AudioPreparationPhase::Vacant;
                    } else {
                        self.phase = AudioPreparationPhase::FailedIncomplete;
                    }
                }
                return Err(Box::new(AudioArrangementStartRefusal { reason, capacity }));
            }
        }
        Ok(ReservedStages {
            slots,
            capacity,
            next,
            signal_graph: effects.is_some() || project.has_group_tracks(),
        })
    }
    fn construct_reserved_on_control(
        &mut self,
        project: &hex_project::Project,
        inputs: Option<OwnedAudioInputs>,
        stages: Stages,
        reserved: ReservedStages,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> AudioPreparationToken {
        let ReservedStages {
            slots,
            capacity,
            next,
            signal_graph,
        } = reserved;
        let Stages {
            include_session,
            assemble_session,
            assemble_arrangement,
            assemble_tracks,
            assemble_returns,
            assemble_preview,
            assemble_project,
            assemble_revision,
        } = stages;
        // Vacant destination validated above. Fixed moves only; no owner dropped.
        self.slots = slots;
        self.bank = inputs;
        self.incarnation = next;
        self.capacity = capacity;
        self.phase = AudioPreparationPhase::Constructing;

        let result = catch_unwind(AssertUnwindSafe(|| {
            self.construct(project, cancel)?;
            if assemble_arrangement {
                self.arrangement_clips.construct(
                    &mut self.slots,
                    self.bank
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?,
                    assemble_tracks,
                    assemble_returns,
                    assemble_preview,
                )?;
            }
            if include_session {
                self.session.construct(
                    self.bank
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?,
                )?;

                self.structure.validate(&self.session, true)?;
                self.tempo.construct()?;
                self.shared.construct()?;
                if assemble_session {
                    self.assembly.construct(
                        &mut self.session,
                        &mut self.structure,
                        &mut self.tempo,
                        &mut self.shared,
                        self.bank
                            .as_ref()
                            .ok_or(AudioStorageError::SourceMismatch)?,
                    )?;
                }
            }
            if assemble_project {
                self.arrangement_clips.construct_root(&mut self.shared)?;
                if signal_graph {
                    self.arrangement_clips.prepare_signal_graph(project)?;
                }
            }
            if assemble_project {
                self.arrangement_clips.validate_root_sources(
                    &self.slots,
                    self.bank
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?,
                )?;
            }
            if assemble_revision {
                self.revision.construct(
                    &mut self.arrangement_clips,
                    &mut self.assembly,
                    crate::PreparedSourceRouting::prepare(project)
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                )?;
            }

            let view = self.revision.storage_view()?;
            if assemble_revision {
                let bank = self
                    .bank
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                self.arrangement_clips.validate_root_sources_at(
                    &self.slots,
                    bank,
                    view.arrangement,
                )?;
                self.assembly.validate_pairs_at(bank, view.session)?;
            }
            transformed::check_cancel(cancel)?;
            self.capacity.revision_roots = self.revision.capacity()?;

            self.capacity.arrangement_clips =
                self.arrangement_clips
                    .capacity_at(true, view.arrangement, view.preview)?;

            self.capacity.assembly = self.assembly.capacity_at(true, view.session)?;

            self.capacity.shared = self.shared.observe_at(
                self.arrangement_clips.root_context_at(view.arrangement),
                self.assembly.shared_storage_at(view.session)?,
            )?;
            if include_session && self.capacity.shared.installed_handle_obligations != 3 {
                return Err(AudioStorageError::UnknownStorage);
            }
            if self.current_sampler.metadata().is_some() {
                self.current_sampler.wrap_bank(&mut self.bank)?;
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => {
                if include_session {
                    // Infallible ownership-location shift; totals were measured
                    // before bank movement, and the same two buffers moved.
                    self.capacity.tempo.prepared_point_capacity =
                        self.capacity.tempo.copy_point_capacity;
                    self.capacity.tempo.prepared_prefix_capacity =
                        self.capacity.tempo.copy_prefix_capacity;
                    self.capacity.tempo.prepared_backing_bytes =
                        self.capacity.tempo.copy_backing_bytes;
                    self.capacity.tempo.copy_point_capacity = 0;
                    self.capacity.tempo.copy_prefix_capacity = 0;
                    self.capacity.tempo.copy_backing_bytes = 0;
                    self.capacity.shared.installed_handle_obligations = 3;
                }
                self.phase = if include_session {
                    AudioPreparationPhase::ArrangementAndSessionSources
                } else {
                    AudioPreparationPhase::ArrangementSources
                }
            }
            Ok(Err(reason)) => {
                self.reason = Some(reason);
                self.phase = AudioPreparationPhase::Cleaning;
            }
            Err(payload) => {
                self.first_panic = Some(payload); // only possible first invocation
                self.phase = AudioPreparationPhase::FailedIncomplete;
            }
        }
        AudioPreparationToken {
            root: self.root,
            incarnation: next,
        }
    }
    fn preflight(
        &self,
        project: &hex_project::Project,
        generation: u64,
        inputs: &OwnedAudioInputs,
        slots: &mut Vec<SourceSlot>,
        capacity: &mut AudioArrangementCapacity,
        resources: ConstructorResources<'_>,
    ) -> Result<u64, AudioStorageError> {
        let effects = resources.effects;
        if let Some(input) = resources.native_input
            && crate::device_automation::native_parameter_input_target(
                project,
                input.device(),
                input.target(),
            )
            .map_err(|_| AudioStorageError::UnsupportedGraph)?
                != input
        {
            return Err(AudioStorageError::SourceMismatch);
        }

        let allow_sampler = resources.sampler.is_some();
        if self.phase != AudioPreparationPhase::Vacant
            || self.bank.is_some()
            || !self.slots.is_empty()
        {
            return Err(AudioStorageError::Capacity);
        }
        let next = self
            .incarnation
            .checked_add(1)
            .ok_or(AudioStorageError::HandleExhausted)?;
        if generation == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        validate_constructor_project_with_effects(project, effects)?;
        if allow_sampler
            && inputs
                .sources
                .iter()
                .any(|(_, source)| !matches!(source.kind, SourceKind::Resident(_)))
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        if !allow_sampler
            && crate::native_device_resources::current_sampler::validate_project_with_effects(
                project,
                effects.is_some(),
            )?
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let count = project
            .tracks()
            .iter()
            .try_fold(0_usize, |n, t| n.checked_add(t.clips().len()))
            .ok_or(AudioStorageError::Overflow)?;
        if inputs
            .owning_handle_count()
            .checked_add(count)
            .and_then(|n| n.checked_add(transformed::transformed_count(project)))
            .ok_or(AudioStorageError::Overflow)?
            > inputs.reservation.handle_obligations
        {
            return Err(AudioStorageError::Capacity);
        }
        slots
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        capacity.source_slot_capacity = slots.capacity();
        capacity.source_slot_bytes = slots
            .capacity()
            .checked_mul(mem::size_of::<SourceSlot>())
            .ok_or(AudioStorageError::Overflow)?;
        capacity.previous_slot_backing_bytes = self
            .slots
            .capacity()
            .checked_mul(mem::size_of::<SourceSlot>())
            .ok_or(AudioStorageError::Overflow)?;
        capacity.custodian_inline_bytes = mem::size_of::<Self>();
        capacity.input_container_bytes = inputs
            .capacity
            .source_container_bytes
            .checked_add(inputs.capacity.alias_container_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        // Empty retained Session Vec backings still belong to this root even
        // when a later opt-in request compiles Arrangement sources only.
        capacity.session = self.session.retained_capacity()?;
        capacity.structure = self.structure.observe()?;
        capacity.tempo = self.tempo.observe(false)?;
        capacity.shared = self.shared.observe_at(
            self.arrangement_clips.root_context(),
            self.assembly.shared_storage()?,
        )?;
        capacity.assembly = self.assembly.capacity(false)?;
        capacity.arrangement_clips = self.arrangement_clips.capacity(false)?;
        capacity.revision_roots = self.revision.capacity()?;
        for (index, (_, source)) in inputs.sources.iter().enumerate() {
            let (payload, overhead) = crate::audio_input_storage::parts(source)?;
            if !inputs.sources[..index]
                .iter()
                .any(|(_, old)| crate::audio_input_storage::same(old, source))
            {
                capacity.declared_source_bytes = capacity
                    .declared_source_bytes
                    .checked_add(payload)
                    .and_then(|n| n.checked_add(overhead))
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        let (derived_retained, derived_workspace) =
            transformed::transformed_audio_quote_for(project, &|clip| {
                transformed::pitched_source_index(project, inputs, clip).is_some()
                    || transformed::repitch_source_index(project, inputs, clip).is_some()
                    || transformed::granular_source_index(project, inputs, clip).is_some()
            })?;
        capacity.declared_source_bytes = capacity
            .declared_source_bytes
            .checked_add(derived_retained)
            .ok_or(AudioStorageError::Overflow)?;
        capacity.native_compiler_workspace_bytes = derived_workspace;
        capacity.retained_stage_bytes = capacity
            .source_slot_bytes
            .checked_add(capacity.custodian_inline_bytes)
            .and_then(|n| n.checked_add(capacity.input_container_bytes))
            .and_then(|n| n.checked_add(capacity.declared_source_bytes))
            .and_then(|n| capacity.session.bytes().ok().and_then(|m| n.checked_add(m)))
            .and_then(|n| {
                capacity
                    .structure
                    .bytes()
                    .ok()
                    .and_then(|m| n.checked_add(m))
            })
            .and_then(|n| capacity.tempo.bytes().ok().and_then(|m| n.checked_add(m)))
            .and_then(|n| capacity.shared.bytes().ok().and_then(|m| n.checked_add(m)))
            .and_then(|n| {
                capacity
                    .assembly
                    .bytes()
                    .ok()
                    .and_then(|m| n.checked_add(m))
            })
            .and_then(|n| {
                n.checked_add(capacity.arrangement_clips.bytes().ok()?)
                    .and_then(|n| n.checked_add(capacity.revision_roots.bytes()))
            })
            .and_then(|n| n.checked_add(inputs.reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        capacity.construction_peak_bytes = capacity
            .retained_stage_bytes
            .checked_add(inputs.reservation.caller_construction_bytes)
            .and_then(|n| n.checked_add(capacity.previous_slot_backing_bytes))
            .and_then(|n| n.checked_add(derived_workspace))
            .ok_or(AudioStorageError::Overflow)?;
        if capacity.retained_stage_bytes > inputs.reservation.max_prepared_data_bytes
            || capacity.construction_peak_bytes > inputs.reservation.max_construction_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        if effects.is_some() || resources.native_input.is_some() {
            let mut retained = 0usize;
            let mut overlap = 0usize;
            for devices in project
                .tracks()
                .iter()
                .map(|t| t.devices())
                .chain(project.return_tracks().iter().map(|r| r.devices()))
                .chain(std::iter::once(project.main_devices()))
            {
                let (data, scratch) = arrangement_clips::chains::ChainCustody::default()
                    .quote_with_input(devices, project.sample_rate(), resources.native_input)?;
                retained = retained
                    .checked_add(data)
                    .ok_or(AudioStorageError::Overflow)?;
                overlap = overlap
                    .checked_add(scratch)
                    .ok_or(AudioStorageError::Overflow)?;
            }
            if capacity
                .retained_stage_bytes
                .checked_add(retained)
                .is_none_or(|n| n > inputs.reservation.max_prepared_data_bytes)
                || capacity
                    .construction_peak_bytes
                    .checked_add(retained)
                    .and_then(|n| n.checked_add(overlap))
                    .is_none_or(|n| n > inputs.reservation.max_construction_data_bytes)
            {
                return Err(AudioStorageError::ReservationExceeded);
            }
        }
        for track in project.tracks() {
            for clip in track.clips() {
                let source = inputs
                    .sources
                    .iter()
                    .position(|(id, _)| *id == clip.asset())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let asset = project
                    .asset(clip.asset())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let owner = &inputs.sources[source].1;
                if owner.sample_rate() != project.sample_rate()
                    || owner.frames() != asset.playback_frames()
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
                let pitch_source = transformed::pitched_source_index(project, inputs, clip);
                let repitch_source = transformed::repitch_source_index(project, inputs, clip);
                let granular_source = transformed::granular_source_index(project, inputs, clip);
                let derived_source = pitch_source.or(repitch_source).or(granular_source);
                let transformed = clip.audio_edit().is_some() && derived_source.is_none();
                if derived_source.is_none() {
                    transformed::validate_sources(project, inputs, clip)?;
                }
                let source = derived_source.unwrap_or(source);
                let derived_view = if repitch_source.is_some() || granular_source.is_some() {
                    let edit = clip.audio_edit().ok_or(AudioStorageError::SourceMismatch)?;
                    let (_, _, start, end) = edit
                        .projected_domain(project.tempo_map())
                        .map_err(|_| AudioStorageError::SourceMismatch)?;
                    let derived_frames = inputs.sources[source].1.frames();
                    if start >= end || end > derived_frames {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                    Some((start, end - start))
                } else {
                    None
                };
                let end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(AudioStorageError::Overflow)?;
                let source_end = clip
                    .source_offset()
                    .checked_add(clip.source_length())
                    .ok_or(AudioStorageError::Overflow)?;
                if clip.duration() == 0
                    || clip.source_length() == 0
                    || (!transformed && source_end > owner.frames())
                    || (!clip.looping() && clip.source_phase() != 0)
                    || (clip.looping() && clip.source_phase() >= clip.source_length())
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                clip.fade()
                    .validate_segment(clip.duration())
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                slots.push(SourceSlot {
                    active: clip.active(),
                    track: track.id(),
                    clip: clip.id(),
                    source,
                    handle: handle()?,
                    start: clip.start(),
                    end,
                    source_offset: usize::try_from(if transformed {
                        0
                    } else if let Some((start, _)) = derived_view {
                        start
                    } else {
                        clip.source_offset()
                    })
                    .map_err(|_| AudioStorageError::Overflow)?,
                    source_length: usize::try_from(if transformed {
                        clip.duration()
                    } else if let Some((_, length)) = derived_view {
                        length
                    } else {
                        clip.source_length()
                    })
                    .map_err(|_| AudioStorageError::Overflow)?,
                    phase: if transformed || derived_view.is_some() {
                        0
                    } else {
                        clip.source_phase()
                    },
                    gain: clip.gain().linear(),
                    looping: !transformed && derived_view.is_none() && clip.looping(),
                    fade: if pitch_source.is_some() {
                        clip.audio_edit()
                            .ok_or(AudioStorageError::SourceMismatch)?
                            .domain
                            .fade
                    } else if transformed || derived_view.is_some() {
                        hex_project::ClipFade::neutral(clip.duration())
                            .map_err(|_| AudioStorageError::SourceMismatch)?
                    } else {
                        clip.fade()
                    },
                    owner: None,
                    prepared_origin: None,
                    sparse_frames: crate::audio_transform::sparse_group_plan(clip)
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?
                        .map(|(_, frames, _)| frames),
                    sparse: None,
                    transformed,
                    disposal: Disposal::Unstarted,
                    #[cfg(test)]
                    probe: None,
                });
            }
        }
        Ok(next)
    }
    fn construct(
        &mut self,
        project: &hex_project::Project,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<(), AudioStorageError> {
        for index in 0..self.slots.len() {
            transformed::check_cancel(cancel)?;
            // Handle obligation and exact destination existed before the clone.
            let slot = &mut self.slots[index];
            let bank = self
                .bank
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            if slot.transformed {
                transformed::construct_origin(project, bank, slot)?;
                transformed::check_cancel(cancel)?;
            }
            let source = slot.source_owner(bank)?;
            let kind = match &source.kind {
                SourceKind::Resident(audio) => SourceKind::Resident(audio.clone()),
                SourceKind::Streamed(meta) => SourceKind::Streamed(meta.clone_on_control()),
            };
            slot.owner = Some(OwnedAudioSource {
                kind,
                handle: slot.handle,
            });
            #[cfg(test)]
            {
                assert_ne!(
                    self.panic_after,
                    Some(index),
                    "construction panic after installed alias"
                );
                if self.fail_after == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
            }
        }
        Ok(())
    }
    /// Cancellation is scalar only. The host's lifecycle cleanup tick disposes
    /// owners; token Drop/ordinary handle abandonment cannot perform cleanup.
    pub fn cancel(&mut self, token: &AudioPreparationToken) -> bool {
        if !self.matches(token) || self.phase == AudioPreparationPhase::FailedIncomplete {
            return false;
        }
        self.phase = AudioPreparationPhase::Cleaning;
        true
    }
    pub fn close_on_control(&mut self) {
        if !matches!(
            self.phase,
            AudioPreparationPhase::Vacant
                | AudioPreparationPhase::FailedIncomplete
                | AudioPreparationPhase::Recoverable
        ) {
            self.phase = AudioPreparationPhase::Cleaning;
        }
    }
    /// CONTROL ONLY, outside ordinary locks. Pending is recorded before take;
    /// panic makes an irreversible tombstone, preserving bank and remaining rows.
    pub fn cleanup_on_control(&mut self) {
        if self.phase != AudioPreparationPhase::Cleaning {
            return;
        }
        if !self.dispose_arrangement_clips_on_control() || !self.dispose_assembly_on_control() {
            return;
        }
        while self.next_disposal < self.slots.len() {
            let index = self.next_disposal;
            if !matches!(
                self.slots[index].disposal,
                Disposal::Unstarted | Disposal::Transferred
            ) {
                self.phase = AudioPreparationPhase::FailedIncomplete;
                return;
            }
            self.slots[index].disposal = Disposal::Pending;
            let result = catch_unwind(AssertUnwindSafe(|| {
                drop(self.slots[index].owner.take());
                drop(self.slots[index].prepared_origin.take());
                drop(self.slots[index].sparse.take());
                #[cfg(test)]
                drop(self.slots[index].probe.take());
            }));
            match result {
                Ok(()) => {
                    self.slots[index].disposal = Disposal::Complete;
                    self.next_disposal += 1;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return;
                }
            }
        }
        if !self.dispose_session_on_control() {
            return;
        }
        if !self.dispose_structure_on_control() {
            return;
        }
        if !self.dispose_tempo_on_control() {
            return;
        }
        if !self.dispose_shared_on_control() {
            return;
        }
        self.phase = AudioPreparationPhase::Recoverable;
    }
    fn dispose_arrangement_clips_on_control(&mut self) -> bool {
        match catch_unwind(AssertUnwindSafe(|| {
            self.revision
                .restore(&mut self.arrangement_clips, &mut self.assembly)
        })) {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                self.phase = AudioPreparationPhase::FailedIncomplete;
                return false;
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = AudioPreparationPhase::FailedIncomplete;
                return false;
            }
        }
        if self
            .arrangement_clips
            .restore_root(&mut self.shared)
            .is_err()
        {
            self.phase = AudioPreparationPhase::FailedIncomplete;
            return false;
        }
        while self.arrangement_clips.has_next() {
            match catch_unwind(AssertUnwindSafe(|| self.arrangement_clips.dispose_next())) {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
            }
        }
        true
    }
    fn dispose_assembly_on_control(&mut self) -> bool {
        while self.assembly.has_next_disposal() {
            match catch_unwind(AssertUnwindSafe(|| self.assembly.dispose_next())) {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
            }
        }
        if self.assembly.return_timing(&mut self.tempo).is_err() {
            self.phase = AudioPreparationPhase::FailedIncomplete;
            return false;
        }
        true
    }
    fn dispose_session_on_control(&mut self) -> bool {
        while self.session.has_next_disposal() {
            match catch_unwind(AssertUnwindSafe(|| self.session.dispose_next())) {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
            }
        }
        true
    }
    fn dispose_structure_on_control(&mut self) -> bool {
        while self.structure.has_next_disposal() {
            match catch_unwind(AssertUnwindSafe(|| self.structure.dispose_next())) {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
            }
        }
        true
    }
    fn dispose_tempo_on_control(&mut self) -> bool {
        match catch_unwind(AssertUnwindSafe(|| self.tempo.cleanup())) {
            Ok(Ok(())) => true,
            Ok(Err(_)) => {
                self.phase = AudioPreparationPhase::FailedIncomplete;
                false
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = AudioPreparationPhase::FailedIncomplete;
                false
            }
        }
    }
    fn dispose_shared_on_control(&mut self) -> bool {
        while self.shared.has_next_disposal() {
            match catch_unwind(AssertUnwindSafe(|| self.shared.dispose_next())) {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
                Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = AudioPreparationPhase::FailedIncomplete;
                    return false;
                }
            }
        }
        true
    }
    /// Only actual normal partial-owner disposal permits original-bank recovery.
    /// Identity/refusal leaves the input bank, its Vec capacities and tokens intact.
    pub fn recover_on_control(
        &mut self,
        token: &AudioPreparationToken,
    ) -> Option<OwnedAudioInputs> {
        if !self.matches(token) || self.phase != AudioPreparationPhase::Recoverable {
            return None;
        }
        if self.arrangement_clips.clear_disposed().is_err()
            || self.assembly.clear_disposed().is_err()
        {
            return None;
        }
        // Scalar validation precedes original-owner extraction. Clearing disposed
        // rows frees no source/name allocation (only already-empty headers).
        if self.session.clear_disposed_rows().is_err() {
            return None;
        }
        if self.structure.clear_disposed_rows().is_err() {
            return None;
        }
        if self.shared.clear_disposed().is_err() {
            return None;
        }
        if self.current_sampler.restore_bank(&mut self.bank).is_err()
            || self.current_sampler.clear_metadata().is_err()
        {
            return None;
        }
        let bank = self.bank.take()?;
        self.slots.clear(); // normally disposed scalar rows; backing stays charged
        self.next_disposal = 0;
        self.reason = None;
        self.phase = AudioPreparationPhase::Vacant;
        Some(bank)
    }
    pub fn audio_prepared_revision_metadata(&self) -> Option<crate::AudioPreparedRevisionMetadata> {
        if self.phase != AudioPreparationPhase::ArrangementAndSessionSources {
            return None;
        }
        self.revision.metadata()
    }
    pub fn audio_preview_metadata(&self) -> Option<crate::AudioPreviewMetadata> {
        if self.phase != AudioPreparationPhase::ArrangementAndSessionSources {
            return None;
        }
        self.arrangement_clips
            .preview_metadata_at(self.revision.storage_view().ok()?.preview)
    }
    pub fn arrangement_return_metadata(&self) -> Option<ArrangementReturnMetadata<'_>> {
        if self.phase != AudioPreparationPhase::ArrangementAndSessionSources {
            return None;
        }
        self.arrangement_clips
            .return_metadata_at(self.revision.storage_view().ok()?.arrangement)
    }
    pub fn arrangement_track_metadata(&self, index: usize) -> Option<ArrangementTrackMetadata<'_>> {
        if self.phase != AudioPreparationPhase::ArrangementAndSessionSources {
            return None;
        }
        self.arrangement_clips
            .track_metadata_at(index, self.revision.storage_view().ok()?.arrangement)
    }
    pub fn session_inventory_metadata(&self) -> Option<crate::SessionInventoryMetadata<'_>> {
        if self.phase != AudioPreparationPhase::ArrangementAndSessionSources {
            return None;
        }
        self.assembly
            .metadata_at(self.revision.storage_view().ok()?.session)
    }
    pub fn installed_session_source_bindings(&self) -> (usize, usize) {
        let (clips, cache) = self.session.counts();
        (
            clips
                + self.assembly.installed_clip_handles_at(
                    self.revision
                        .storage_view()
                        .ok()
                        .and_then(|view| view.session),
                ),
            cache,
        )
    }
    /// Structural metadata counts only: not a renderable/prepared Session or
    /// storage/lifetime/admission completeness capability.
    pub fn session_structure_counts(&self) -> (usize, usize, usize) {
        self.structure.counts()
    }
    /// Scalar audit of this compiler stage, not complete storage/admission.
    pub fn installed_source_bindings(&self) -> usize {
        self.arrangement_clips.installed_at(
            self.revision
                .storage_view()
                .ok()
                .and_then(|view| view.arrangement),
        ) + self
            .slots
            .iter()
            .filter(|slot| {
                let geometry = slot.end > slot.start
                    && slot.source_length > 0
                    && (!slot.looping || slot.phase < (slot.source_length as u64));
                // IDs/offset are retained exact location/geometry, not alias identity.
                let _location = (slot.track, slot.clip, slot.source_offset, slot.fade);
                geometry
                    && slot
                        .owner
                        .as_ref()
                        .is_some_and(|owner| owner.handle == slot.handle)
            })
            .count()
    }
}

#[cfg(test)]
pub(crate) fn candidate_test_project() -> hex_project::Project {
    tests::fixture(1).0
}

#[cfg(test)]
pub(crate) fn candidate_test_fixture(
    streamed: bool,
) -> Option<(AudioPreparationCustody, AudioPreparationToken, usize, usize)> {
    if !session_source_tests::supported("actual_candidate_disposal") {
        return None;
    }
    let (project, bank, _) = arrangement_clip_tests::fixture(streamed, false, 2);
    let pointer = bank.sources.as_ptr() as usize;
    let capacity = bank.sources.capacity();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            AudioArrangementInput::new(&project, 11, bank).with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    Some((custody, token, pointer, capacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn group_tracks_admit_compilation_and_supported_native_preparation() {
        use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "child".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        assert!(validate_constructor_project(editor.project()).is_ok());
        assert!(crate::compile::project(editor.project(), &Default::default()).is_ok());
        editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        assert!(validate_constructor_project(editor.project()).is_ok());
        assert!(crate::compile::project(editor.project(), &Default::default()).is_ok());
        let effects = hex_devices::native_registry().unwrap();
        assert!(
            validate_constructor_project_with_effects(editor.project(), Some(&effects)).is_ok()
        );
    }
    #[test]
    fn rooted_bank_reservation_refusal_preserves_exact_bank_then_success_transfers_once() {
        if !constructor_supported("rooted_preparation_input") {
            return;
        }
        for streamed in [false, true] {
            let (project, bank, _) = arrangement_clip_tests::fixture(streamed, false, 2);
            let identity = bank.test_bank_identity();
            let mut bank = Some(bank);
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            let context = AudioSourceContext {
                project_generation: 7,
                editor_revision: 42,
            };
            let refusal = custody
                .begin_prepared_revision_in_place_on_control(&project, 0, context, &mut bank)
                .err()
                .unwrap();
            assert_eq!(refusal.reason, AudioStorageError::SourceMismatch);
            assert_eq!(bank.as_ref().unwrap().test_bank_identity(), identity);
            assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
            let token = custody
                .begin_prepared_revision_in_place_on_control(&project, 11, context, &mut bank)
                .unwrap();
            assert!(bank.is_none());
            assert_eq!(
                custody.bank.as_ref().unwrap().test_bank_identity(),
                identity
            );
            assert_eq!(custody.source_context(), Some(context));
            let mut draft = crate::AudioCandidateDraft::new_on_control();
            draft.prepare_on_control(&custody, &token).unwrap();
            let mut custody = Some(custody);
            let mut token = Some(token);
            let mut owner = None;
            draft
                .finish_on_control(&mut custody, &mut token, &mut owner)
                .unwrap();
            assert!(custody.is_none() && token.is_none());
            let mut owner = owner.unwrap();
            owner.dispose_on_control().unwrap();
            assert_eq!(
                owner
                    .recover_inputs_on_control()
                    .unwrap()
                    .test_bank_identity(),
                identity
            );
        }
    }
    #[test]
    fn rooted_bank_survives_reservation_unwind_and_partial_constructor_failure() {
        if !constructor_supported("rooted_preparation_unwind") {
            return;
        }
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let (project, bank, _) = arrangement_clip_tests::fixture(false, false, 2);
        let identity = bank.test_bank_identity();
        let mut bank = Some(bank);
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let context = AudioSourceContext {
            project_generation: 2,
            editor_revision: 9,
        };
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = drops.clone();
        assert_eq!(
            custody
                .begin_prepared_revision_with(&project, 11, context, &mut bank, || {
                    std::panic::panic_any(Payload(payload))
                })
                .err()
                .unwrap()
                .reason,
            AudioStorageError::UnknownStorage
        );
        assert_eq!(bank.as_ref().unwrap().test_bank_identity(), identity);
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let first = custody.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const ();
        assert!(
            custody
                .begin_prepared_revision_in_place_on_control(&project, 11, context, &mut bank)
                .is_err()
        );
        custody.close_on_control();
        custody.cleanup_on_control();
        assert_eq!(
            custody.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const (),
            first
        );
        assert_eq!(bank.as_ref().unwrap().test_bank_identity(), identity);
        drop(custody);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        custody.panic_after = Some(1);
        let token = custody
            .begin_prepared_revision_in_place_on_control(&project, 11, context, &mut bank)
            .unwrap();
        assert!(bank.is_none());
        assert!(custody.matches(&token));
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(
            custody.bank.as_ref().unwrap().test_bank_identity(),
            identity
        );
        custody.close_on_control();
        custody.cleanup_on_control();
        assert!(custody.recover_on_control(&token).is_none());
        assert_eq!(
            custody.bank.as_ref().unwrap().test_bank_identity(),
            identity
        );
    }
    fn constructor_supported(test: &str) -> bool {
        if crate::pcm_arc::native_layout_known() {
            eprintln!("hex preparation evidence: {test}; branch=supported-constructor");
            return true;
        }
        let frames = vec![[0.0; 2]; 8];
        let pointer = frames.as_ptr();
        let refusal = OwnedAudioSource::try_resident(48_000, frames)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::UnsupportedConstructor);
        assert_eq!(refusal.frames.as_ptr(), pointer);
        eprintln!(
            "hex preparation evidence: {test}; branch=unsupported-refusal; original-owner=preserved"
        );
        false
    }
    pub(super) fn fixture(clips: usize) -> (hex_project::Project, OwnedAudioInputs) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "audio".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "audio.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        for n in 0..clips {
            editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: n as u64 * 8,
                    duration: 8,
                    source_offset: 0,
                    source_length: 8,
                    looping: false,
                })
                .unwrap();
        }
        let source = OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 8]);
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                assert_eq!(error.reason, AudioStorageError::UnsupportedConstructor);
                // Tests calling this fixture are gated on actual constructor support.
                panic!("supported fixture constructor refused");
            }
        };
        let inputs = OwnedAudioInputs::try_new(
            vec![(asset, source)],
            AudioStorageReservation {
                allocation_items: 8,
                handle_obligations: 16, // caller reserve includes fixed inventory handles
                max_prepared_data_bytes: usize::MAX,
                max_construction_data_bytes: usize::MAX,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .unwrap_or_else(|_| panic!("input reservation"));
        (editor.project().clone(), inputs)
    }
    #[test]
    fn every_late_alias_failure_recovers_exact_original_bank_and_live_caller_tokens() {
        if !constructor_supported("every_late_alias_failure") {
            return;
        }
        for after in 0..3 {
            let (project, mut bank) = fixture(3);
            let source = bank
                .source_token(project.assets().next().unwrap().id())
                .unwrap();
            let alias = bank.alias_on_control(&source).unwrap();
            let pointer = bank.sources.as_ptr();
            let capacity = bank.sources.capacity();
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            custody.fail_after = Some(after);
            let token = custody
                .begin_arrangement_on_control(&project, 1, bank)
                .unwrap_or_else(|_| panic!("preflight"));
            assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
            assert_eq!(custody.installed_source_bindings(), after + 1);
            assert!(custody.recover_on_control(&token).is_none());
            custody.cleanup_on_control();
            let bank = custody.recover_on_control(&token).unwrap();
            assert_eq!(bank.sources.as_ptr(), pointer);
            assert_eq!(bank.sources.capacity(), capacity);
            assert_eq!(bank.owning_handle_count(), 2);
            assert!(bank.source_view(&source).is_ok());
            assert!(bank.source_view(&alias).is_ok());
            assert!(custody.recover_on_control(&token).is_none());
        }
        eprintln!(
            "hex preparation evidence: every_late_alias_failure; supported-controls=completed; aliases=3"
        );
    }
    #[test]
    fn capacity_and_generation_refusals_preserve_original_owner_and_tokens() {
        if !constructor_supported("capacity_and_generation_refusal") {
            return;
        }
        let (project, mut bank) = fixture(2);
        let token = bank
            .source_token(project.assets().next().unwrap().id())
            .unwrap();
        let pointer = bank.sources.as_ptr();
        bank.reservation.handle_obligations = 1;
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let refusal = custody
            .begin_arrangement_on_control(&project, 1, bank)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::Capacity);
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert!(refusal.inputs.source_view(&token).is_ok());
        let refusal = custody
            .begin_arrangement_on_control(&project, 0, refusal.inputs)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::SourceMismatch);
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
        eprintln!(
            "hex preparation evidence: capacity_and_generation_refusal; supported-controls=completed"
        );
    }
    #[test]
    fn construction_panic_retains_bank_partial_aliases_and_first_payload_without_retry() {
        if !constructor_supported("construction_panic_retention") {
            return;
        }
        let (project, bank) = fixture(3);
        let pointer = bank.sources.as_ptr();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        custody.panic_after = Some(1);
        let token = custody
            .begin_arrangement_on_control(&project, 1, bank)
            .unwrap_or_else(|_| panic!("preflight"));
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(custody.installed_source_bindings(), 2);
        assert_eq!(custody.bank.as_ref().unwrap().sources.as_ptr(), pointer);
        let payload = custody.first_panic.as_ref().unwrap().as_ref() as *const dyn Any;
        custody.cleanup_on_control();
        assert!(!custody.cancel(&token));
        assert!(custody.recover_on_control(&token).is_none());
        assert!(std::ptr::eq(
            payload,
            custody.first_panic.as_ref().unwrap().as_ref()
        ));
        assert_eq!(custody.installed_source_bindings(), 2);
        eprintln!(
            "hex preparation evidence: construction_panic_retention; supported-controls=completed"
        );
    }
    #[test]
    fn actual_slot_destructor_panic_tombstone_retains_remaining_alias_and_bank() {
        if !constructor_supported("actual_slot_destructor_panic") {
            return;
        }
        let (project, bank) = fixture(2);
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_arrangement_on_control(&project, 1, bank)
            .unwrap_or_else(|_| panic!("preflight"));
        let count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&count);
        let thread = std::thread::current().id();
        custody.slots[0].probe = Some(DropProbe(Some(Box::new(move || {
            assert_eq!(std::thread::current().id(), thread);
            observed.fetch_add(1, Ordering::Relaxed);
            panic!("actual owned slot field destructor");
        }))));
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(custody.slots[0].disposal, Disposal::Pending);
        assert!(custody.slots[1].owner.is_some());
        assert!(custody.bank.is_some());
        assert!(custody.first_panic.is_some());
        custody.cleanup_on_control();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert!(custody.recover_on_control(&token).is_none());
        eprintln!(
            "hex preparation evidence: actual_slot_destructor_panic; supported-controls=completed; destructor-count=1"
        );
    }
    #[test]
    fn grouped_origins_recover_original_bank_after_late_alias_failure_or_cancel() {
        use hex_project::{Command, Editor};
        assert!(crate::pcm_arc::native_layout_known());
        for fail in [false, true] {
            let (project, bank) = fixture(2);
            let identity = bank.test_bank_identity();
            let source = bank
                .source_token(project.assets().next().unwrap().id())
                .unwrap();
            let ids = project.tracks()[0]
                .clips()
                .iter()
                .map(|clip| clip.id())
                .collect();
            let mut editor = Editor::new(project, 8).unwrap();
            editor.execute(Command::GroupClips(ids)).unwrap();
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            custody.fail_after = fail.then_some(0);
            let token = custody
                .begin_arrangement_on_control(editor.project(), 1, bank)
                .unwrap_or_else(|_| panic!("grouped preflight"));
            assert!(custody.slots[0].prepared_origin.is_some());
            assert!(custody.slots[0].owner.is_some());
            assert_eq!(custody.resident_pcm_capacity_frames().unwrap(), 24);
            if fail {
                assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
            } else {
                assert_eq!(custody.phase(), AudioPreparationPhase::ArrangementSources);
                assert!(custody.cancel(&token));
            }
            let caller = std::thread::current().id();
            custody.slots[0].probe = Some(DropProbe(Some(Box::new(move || {
                assert_eq!(std::thread::current().id(), caller);
            }))));
            custody.cleanup_on_control();
            assert!(custody.slots[0].prepared_origin.is_none());
            assert!(custody.slots[0].owner.is_none());
            let recovered = custody.recover_on_control(&token).unwrap();
            assert_eq!(recovered.test_bank_identity(), identity);
            assert!(recovered.source_view(&source).is_ok());
        }
    }
}
