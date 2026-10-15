//! Preparation-only Arrangement and opt-in Session source-binding stages. Not a prepared revision,
//! storage-complete graph, publication admission or lifetime-closure authority.
//! The audio host must retain this custody through cancellation and abandonment.
use crate::audio_only_inputs::*;
#[cfg(test)]
#[path = "session_audio_preparation_tests.rs"]
mod session_source_tests;
#[cfg(test)]
#[path = "session_structure_preparation_tests.rs"]
mod structure_tests;
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
}
pub struct AudioArrangementRefusal {
    pub reason: AudioStorageError,
    pub inputs: OwnedAudioInputs,
    pub capacity: AudioArrangementCapacity,
}
/// Borrowed canonical metadata plus consumed bank; no audio/stream dependency
/// enters the engine. A host need not depend on hex-project to accept this input.
pub struct AudioArrangementInput<'a> {
    project: &'a hex_project::Project,
    generation: u64,
    inputs: OwnedAudioInputs,
    include_session: bool,
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
            include_session: false,
        }
    }
    /// Explicitly add the new persistent Session audio source/name stage.
    /// Previous legacy inventories are never adopted; cache aliases originate
    /// only in this same input bank and remain in the same preparation custodian.
    pub fn with_session_sources(mut self) -> Self {
        self.include_session = true;
        self
    }
    pub fn refuse(self, reason: AudioStorageError) -> Box<AudioArrangementRefusal> {
        Box::new(AudioArrangementRefusal {
            reason,
            inputs: self.inputs,
            capacity: AudioArrangementCapacity::default(),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposal {
    Unstarted,
    Pending,
    Complete,
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
        self.begin_stages_on_control(
            input.project,
            input.generation,
            input.inputs,
            input.include_session,
        )
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
    /// Before bank consumption, only scalar draft rows are allocated. Every
    /// identity, geometry and storage refusal returns the EXACT original bank.
    /// Fatal OOM/abort is outside the recoverable API contract.
    pub fn begin_arrangement_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
        self.begin_stages_on_control(project, generation, inputs, false)
    }
    fn begin_stages_on_control(
        &mut self,
        project: &hex_project::Project,
        generation: u64,
        inputs: OwnedAudioInputs,
        include_session: bool,
    ) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
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
            });
            // Persistent destinations BEFORE fallible names/helper work. Bank
            // remains borrowed, so every pre-transfer refusal returns it intact.
            self.phase = AudioPreparationPhase::Constructing;
            self.structure.capacity.previous_growth_backing_bytes = 0;
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.session
                    .reserve(project, generation, &inputs, slots.len())?;
                self.structure.reserve(project, &self.session)
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
            let result = match result {
                Ok(result) => result.and_then(|()| {
                    structure_observation?;
                    let current = capacity
                        .session
                        .bytes()?
                        .checked_add(capacity.structure.bytes()?)
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
                if self.phase != AudioPreparationPhase::FailedIncomplete {
                    if self.dispose_session_on_control()
                        && self.dispose_structure_on_control()
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
            if include_session {
                self.session.construct(
                    self.bank
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?,
                )?;
                self.structure.validate(&self.session, true)?;
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => {
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
        while self.next_disposal < self.slots.len() {
            let index = self.next_disposal;
            if self.slots[index].disposal != Disposal::Unstarted {
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
        self.phase = AudioPreparationPhase::Recoverable;
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
    /// Only actual normal partial-owner disposal permits original-bank recovery.
    /// Identity/refusal leaves the input bank, its Vec capacities and tokens intact.
    pub fn recover_on_control(
        &mut self,
        token: &AudioPreparationToken,
    ) -> Option<OwnedAudioInputs> {
        if !self.matches(token) || self.phase != AudioPreparationPhase::Recoverable {
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
        let bank = self.bank.take()?;
        self.slots.clear(); // normally disposed scalar rows; backing stays charged
        self.next_disposal = 0;
        self.reason = None;
        self.phase = AudioPreparationPhase::Vacant;
        Some(bank)
    }
    pub fn installed_session_source_bindings(&self) -> (usize, usize) {
        self.session.counts()
    }
    /// Structural metadata counts only: not a renderable/prepared Session or
    /// storage/lifetime/admission completeness capability.
    pub fn session_structure_counts(&self) -> (usize, usize, usize) {
        self.structure.counts()
    }
    /// Scalar audit of this compiler stage, not complete storage/admission.
    pub fn installed_source_bindings(&self) -> usize {
        self.slots
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
                handle_obligations: 8,
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
