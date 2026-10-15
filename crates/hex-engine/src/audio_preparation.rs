//! Preparation-only Arrangement and opt-in sealed Session/revision assembly.
//! Not a storage-complete graph, publication admission or lifetime authority.
//! The audio host must retain this custody through cancellation and abandonment.
use crate::audio_only_inputs::*;
mod arrangement_clips;
pub(crate) use arrangement_clips::ArrangementClips;
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
}
pub struct AudioArrangementRefusal {
    pub reason: AudioStorageError,
    pub inputs: OwnedAudioInputs,
    pub capacity: AudioArrangementCapacity,
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
/// Borrowed canonical metadata plus consumed bank; no audio/stream dependency
/// enters the engine. A host need not depend on hex-project to accept this input.
pub struct AudioArrangementInput<'a> {
    project: &'a hex_project::Project,
    generation: u64,
    inputs: OwnedAudioInputs,
    stages: Stages,
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
        }
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
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    source: usize,
    handle: u64,
    start: u64,
    end: u64,
    source_offset: usize,
    source_length: usize,
    phase: u64,
    looping: bool,
    fade: hex_project::ClipFade,
    owner: Option<OwnedAudioSource>,
    disposal: Disposal,
    #[cfg(test)]
    probe: Option<DropProbe>,
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
    incarnation: u64,
    phase: AudioPreparationPhase,
    bank: Option<OwnedAudioInputs>,
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
    pub fn begin_input_on_control(
        &mut self,
        input: AudioArrangementInput<'_>,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
        self.begin_stages_on_control(input.project, input.generation, input.inputs, input.stages)
    }
    pub fn try_new_on_control() -> Result<Self, AudioStorageError> {
        Ok(Self {
            root: handle()?,
            incarnation: 0,
            phase: AudioPreparationPhase::Vacant,
            bank: None,
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
        self.begin_stages_on_control(project, generation, inputs, Stages::default())
    }
    fn begin_stages_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
        stages: Stages,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
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
        let result = self.preflight(project, generation, &inputs, &mut slots, &mut capacity);
        let next = match result {
            Ok(next) => next,
            Err(reason) => {
                return Err(Box::new(AudioArrangementRefusal {
                    reason,
                    inputs,
                    capacity,
                }));
            }
        };
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
                self.session
                    .reserve(project, generation, &inputs, slots.len())?;
                self.structure.reserve(project, &self.session)?;
                self.tempo.reserve(project)?;
                if assemble_revision {
                    self.revision.reserve(project)?;
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
                let return_handles = if assemble_returns {
                    project.return_tracks().len()
                } else {
                    0
                };
                let handles = inputs
                    .owning_handle_count()
                    .checked_add(slots.len())
                    .and_then(|n| n.checked_add(self.session.capacity.reserved_handle_destinations))
                    .and_then(|n| n.checked_add(track_handles))
                    .and_then(|n| n.checked_add(return_handles))
                    .and_then(|n| n.checked_add(usize::from(assemble_preview)))
                    .and_then(|n| n.checked_add(if assemble_revision { 4 } else { 0 }))
                    .ok_or(AudioStorageError::Overflow)?;
                self.shared
                    .reserve(handles, inputs.reservation.handle_obligations)?;
                if assemble_session {
                    self.assembly.reserve(
                        &self.structure,
                        &self.session,
                        self.shared.inventory_handles(),
                    )?;
                }
                if assemble_arrangement {
                    self.arrangement_clips
                        .reserve(&slots, &inputs, project, stages)?;
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
                    let current = capacity
                        .session
                        .bytes()?
                        .checked_add(capacity.structure.bytes()?)
                        .and_then(|n| n.checked_add(capacity.tempo.bytes().ok()?))
                        .and_then(|n| n.checked_add(capacity.shared.bytes().ok()?))
                        .and_then(|n| n.checked_add(capacity.assembly.bytes().ok()?))
                        .and_then(|n| {
                            n.checked_add(capacity.arrangement_clips.bytes().ok()?)
                                .and_then(|n| n.checked_add(capacity.revision_roots.bytes()))
                        })
                        .ok_or(AudioStorageError::Overflow)?;
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
                        .ok_or(AudioStorageError::Overflow)?;
                    if capacity.retained_stage_bytes > inputs.reservation.max_prepared_data_bytes
                        || capacity.construction_peak_bytes
                            > inputs.reservation.max_construction_data_bytes
                    {
                        return Err(AudioStorageError::ReservationExceeded);
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
                    {
                        self.phase = AudioPreparationPhase::Vacant;
                    } else {
                        self.phase = AudioPreparationPhase::FailedIncomplete;
                    }
                }
                return Err(Box::new(AudioArrangementRefusal {
                    reason,
                    inputs,
                    capacity,
                }));
            }
        }
        // Vacant destination validated above. Fixed moves only; no owner dropped.
        self.slots = slots;
        self.bank = Some(inputs);
        self.incarnation = next;
        self.capacity = capacity;
        self.phase = AudioPreparationPhase::Constructing;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.construct()?;
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
            }
            if assemble_project {
                self.arrangement_clips.validate_root_sources(
                    self.bank
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?,
                )?;
            }
            if assemble_revision {
                self.revision
                    .construct(&mut self.arrangement_clips, &mut self.assembly)?;
            }
            let view = self.revision.storage_view()?;
            if assemble_revision {
                let bank = self
                    .bank
                    .as_ref()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                self.arrangement_clips
                    .validate_root_sources_at(bank, view.arrangement)?;
                self.assembly.validate_pairs_at(bank, view.session)?;
            }
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
        Ok(AudioPreparationToken {
            root: self.root,
            incarnation: next,
        })
    }
    fn preflight(
        &self,
        project: &hex_project::Project,
        generation: u64,
        inputs: &OwnedAudioInputs,
        slots: &mut Vec<SourceSlot>,
        capacity: &mut AudioArrangementCapacity,
    ) -> Result<u64, AudioStorageError> {
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
        // This first compiler stage has no Session inventory output. Unsupported
        // instruments/effects/MIDI refuse; Session stage is deliberately separate.
        if project.tracks().iter().any(|t| {
            !t.devices().is_empty() || t.instrument().is_some() || !t.midi_clips().is_empty()
        }) || project
            .return_tracks()
            .iter()
            .any(|t| !t.devices().is_empty())
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        crate::gain_envelope::validate_project(project)
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let count = project
            .tracks()
            .iter()
            .try_fold(0_usize, |n, t| n.checked_add(t.clips().len()))
            .ok_or(AudioStorageError::Overflow)?;
        if inputs
            .owning_handle_count()
            .checked_add(count)
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
            .ok_or(AudioStorageError::Overflow)?;
        if capacity.retained_stage_bytes > inputs.reservation.max_prepared_data_bytes
            || capacity.construction_peak_bytes > inputs.reservation.max_construction_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
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
                if let SourceKind::Streamed(meta) = &owner.kind {
                    if meta.key().generation() != generation
                        || meta.key().asset() != asset.id()
                        || meta.source_rate() != asset.source_sample_rate()
                        || meta.source_frames() != asset.source_frames()
                        || meta.conversion_version() != asset.conversion_version()
                    {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                }
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
                    || source_end > owner.frames()
                    || (!clip.looping() && clip.source_phase() != 0)
                    || (clip.looping() && clip.source_phase() >= clip.source_length())
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
                clip.fade()
                    .validate_segment(clip.duration())
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                slots.push(SourceSlot {
                    track: track.id(),
                    clip: clip.id(),
                    source,
                    handle: handle()?,
                    start: clip.start(),
                    end,
                    source_offset: usize::try_from(clip.source_offset())
                        .map_err(|_| AudioStorageError::Overflow)?,
                    source_length: usize::try_from(clip.source_length())
                        .map_err(|_| AudioStorageError::Overflow)?,
                    phase: clip.source_phase(),
                    looping: clip.looping(),
                    fade: clip.fade(),
                    owner: None,
                    disposal: Disposal::Unstarted,
                    #[cfg(test)]
                    probe: None,
                });
            }
        }
        Ok(next)
    }
    fn construct(&mut self) -> Result<(), AudioStorageError> {
        for index in 0..self.slots.len() {
            // Handle obligation and exact destination existed before the clone.
            let slot = &mut self.slots[index];
            let source = &self
                .bank
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?
                .sources[slot.source]
                .1;
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
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
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
            .execute(Command::AddTrack {
                name: "audio".into(),
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
}
