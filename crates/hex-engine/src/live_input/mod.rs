//! Transient control-prepared typing resources. No project/storage state.
use crate::Invalid;
use hex_lattice::{
    NoteGate, PreparedLiveAnalyticNote, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
};
use hex_lattice_contract::NativeDeviceState;
use hex_project::{DeviceId, Project, TrackId};

mod performance;
pub use performance::{TypingApplication, TypingPerformance};

pub const TYPING_PITCHES: usize = 12;
pub const TYPING_VELOCITY: u8 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypingTarget {
    pub project_generation: u64,
    pub project_revision: u64,
    pub track: TrackId,
    pub device: DeviceId,
}
impl TypingTarget {
    /// Canonical project generations start at zero. The transient protocol
    /// reserves zero, so encode the same incarnation with a checked offset.
    /// Track/device IDs are the original canonical IDs, without remapping.
    pub fn owner(self) -> Result<crate::typing_protocol::Owner, Invalid> {
        Ok(crate::typing_protocol::Owner {
            generation: self
                .project_generation
                .checked_add(1)
                .ok_or(Invalid("typing project incarnation exhausted"))?,
            track: self.track.value(),
            device: self.device.value(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypingAdmission {
    pub target: TypingTarget,
    pub owner: crate::typing_protocol::Owner,
    pub epoch: u64,
    pub armed: bool,
    pub allowed_keys: u16,
    pub cut_through_epoch: u64,
}

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // Keep the bounded inline sampler allocation-free.
pub(crate) enum TypingPitch {
    Inline(PreparedLiveAnalyticNote),
    Shared(PreparedSharedAnalyticNote),
    Mutable,
}

/// A bounded immutable palette, prepared only on the control worker. Native
/// mutable entries are validation certificates, not alternate analytic audio.
pub struct PreparedTypingArm {
    target: TypingTarget,
    state: NativeDeviceState,
    sample_rate: u32,
    entries: [Option<TypingPitch>; TYPING_PITCHES],
    context: Option<Box<dyn PreparedSharedAnalyticContext>>,
    mutable: bool,
    voice_limit: usize,
    release_frames: u64,
    context_id: Option<hex_lattice::AnalyticContinuationId>,
    effect_topology: Vec<(DeviceId, hex_lattice_contract::DeviceTypeId, bool)>,
    instrument_position: usize,
}
impl PreparedTypingArm {
    pub(crate) fn topology_matches(&self, source: &Self) -> bool {
        self.target.track == source.target.track
            && self.target.device == source.target.device
            && self.state.type_id() == source.state.type_id()
            && self.mutable == source.mutable
            && self.instrument_position == source.instrument_position
            && self.effect_topology == source.effect_topology
    }
    pub(crate) fn matches_track(&self, track: &crate::Track) -> bool {
        track.canonical_track_id() == Some(self.target.track)
            && track.instrument_id == Some(self.target.device)
            && track.instrument_state.as_ref() == Some(&self.state)
            && !track.instrument_bypassed
    }
    pub fn prepare(project: &Project, target: TypingTarget) -> Result<Self, Invalid> {
        target.owner()?;
        let device = project
            .track(target.track)
            .and_then(|track| track.instrument())
            .filter(|device| device.id() == target.device && !device.bypassed())
            .ok_or(Invalid("typing target instrument is unavailable"))?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("typing native catalog is unavailable"))?;
        let prepared = registry
            .prepare_instrument(device.state(), project.sample_rate())
            .map_err(|_| Invalid("typing instrument state is unsupported"))?;
        let factory = prepared
            .prepare_mutable_runtime()
            .map_err(|_| Invalid("typing mutable preparation failed"))?;
        let mutable = factory.is_some();
        let voice_limit = factory
            .as_ref()
            .map_or(64, |factory| factory.max_active_occurrences());
        let mut release_frames = factory
            .as_ref()
            .map_or(0, |factory| factory.max_release_tail_frames());
        if voice_limit == 0 || voice_limit > 64 {
            return Err(Invalid("typing voice declaration is invalid"));
        }
        let context = if mutable {
            None
        } else {
            prepared
                .prepare_shared_analytic_context()
                .map_err(|_| Invalid("typing shared preparation failed"))?
        };
        if let Some(context) = context.as_ref() {
            if !context.supports_live_gate() {
                return Err(Invalid("typing open gate is unsupported"));
            }
            if context.declared_context_bytes() > hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES {
                return Err(Invalid("typing shared context exceeds its budget"));
            }
        }
        let mut entries = [None; TYPING_PITCHES];
        for (index, entry) in entries.iter_mut().enumerate() {
            let pitch = 60 + index as u8;
            if mutable {
                if prepared.validate_event_note(pitch, TYPING_VELOCITY).is_ok() {
                    *entry = Some(TypingPitch::Mutable);
                }
            } else if let Some(context) = context.as_ref() {
                if let Ok(note) = context.prepare_note(pitch, TYPING_VELOCITY) {
                    release_frames = release_frames.max(note.release_frames());
                    *entry = Some(TypingPitch::Shared(note));
                }
            } else if let Ok(note) = prepared.prepare_live_note(pitch, TYPING_VELOCITY) {
                release_frames = release_frames.max(note.release_frames());
                *entry = Some(TypingPitch::Inline(note));
            }
        }
        if entries.iter().all(Option::is_none) {
            return Err(Invalid("typing target has no supported pitches"));
        }
        let context_id = context
            .as_ref()
            .map(|_| crate::shared_analytic::SharedContextId::fresh()?.continuation_id())
            .transpose()?;
        let track = project
            .track(target.track)
            .ok_or(Invalid("typing track disappeared"))?;
        let mut effect_topology = Vec::new();
        effect_topology
            .try_reserve_exact(track.devices().len())
            .map_err(|_| Invalid("typing topology allocation failed"))?;
        effect_topology.extend(
            track
                .devices()
                .iter()
                .map(|device| (device.id(), device.state().type_id(), device.bypassed())),
        );
        Ok(Self {
            target,
            state: device.state().clone(),
            sample_rate: project.sample_rate(),
            entries,
            context,
            mutable,
            voice_limit,
            release_frames,
            context_id,
            effect_topology,
            instrument_position: track
                .instrument_position()
                .ok_or(Invalid("typing instrument position disappeared"))?,
        })
    }
    pub fn target(&self) -> TypingTarget {
        self.target
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub(crate) fn shared_context_bytes(&self) -> usize {
        self.context
            .as_ref()
            .map_or(0, |context| context.declared_context_bytes())
    }
    pub fn is_mutable(&self) -> bool {
        self.mutable
    }
    pub fn voice_limit(&self) -> usize {
        self.voice_limit
    }
    pub fn release_frames(&self) -> u64 {
        self.release_frames
    }
    pub fn allowed_keys(&self) -> u16 {
        self.entries
            .iter()
            .enumerate()
            .fold(0, |mask, (index, entry)| {
                mask | if entry.is_some() { 1 << index } else { 0 }
            })
    }
    pub fn matches_project(&self, project: &Project) -> bool {
        project.sample_rate() == self.sample_rate
            && project
                .track(self.target.track)
                .and_then(|track| track.instrument())
                .is_some_and(|device| {
                    device.id() == self.target.device
                        && !device.bypassed()
                        && device.state() == &self.state
                })
    }
    /// Include palette, captured payload, context and ownership reference words.
    /// Runtime ledgers/scratch/acks are separate charges, not excluded costs.
    pub fn declared_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.state.payload().len())?
            .checked_add(4 * std::mem::size_of::<usize>())?
            .checked_add(
                self.effect_topology
                    .capacity()
                    .checked_mul(std::mem::size_of::<(
                        DeviceId,
                        hex_lattice_contract::DeviceTypeId,
                        bool,
                    )>())?,
            )?
            .checked_add(
                self.context
                    .as_ref()
                    .map_or(0, |context| context.declared_context_bytes()),
            )
    }
    pub fn sample(&self, key: usize, age: u64, gate: NoteGate) -> Result<[f32; 2], Invalid> {
        match self
            .entries
            .get(key)
            .copied()
            .flatten()
            .ok_or(Invalid("typing pitch is unavailable"))?
        {
            TypingPitch::Inline(note) => Ok(note.sample(age, gate)),
            TypingPitch::Shared(note) => self
                .context
                .as_ref()
                .ok_or(Invalid("typing context is unavailable"))?
                .sample_live(&note, age, gate)
                .map_err(|_| Invalid("typing shared note was refused")),
            TypingPitch::Mutable => Err(Invalid("typing mutable note needs its native runtime")),
        }
    }
    pub(crate) fn transition_from(
        &self,
        source: &Self,
        previous: Option<hex_lattice::AnalyticContinuation>,
        frame: u64,
    ) -> Result<Option<hex_lattice::AnalyticContinuation>, Invalid> {
        if self.mutable || source.mutable {
            return Ok(None);
        }
        let (Some(target), Some(source_context), Some(target_id), Some(source_id)) = (
            self.context.as_ref(),
            source.context.as_ref(),
            self.context_id,
            source.context_id,
        ) else {
            if self.state != source.state {
                return Err(Invalid("typing inline state cannot carry held notes"));
            }
            return Ok(None);
        };
        let transition = crate::shared_analytic::context_transition(
            target.as_ref(),
            target_id,
            source_context.as_ref(),
            source_id,
            self.state == source.state,
            previous,
            frame,
            self.sample_rate,
        )?;
        if self.state != source.state && transition.is_none() {
            return Err(Invalid(
                "typing provider refuses this held-note state change",
            ));
        }
        if let Some(provider) = target.analytic_continuation() {
            for entry in source.entries.iter().flatten() {
                if let TypingPitch::Shared(note) = entry {
                    provider
                        .validate_note(note)
                        .map_err(|_| Invalid("typing retained note is incompatible"))?;
                }
            }
        }
        Ok(transition)
    }
    pub(crate) fn sample_continued(
        &self,
        key: usize,
        absolute: u64,
        age: u64,
        gate: NoteGate,
        transition: Option<hex_lattice::AnalyticContinuation>,
    ) -> Result<[f32; 2], Invalid> {
        let Some(transition) = transition else {
            return self.sample(key, age, gate);
        };
        let Some(TypingPitch::Shared(note)) = self.entries.get(key).copied().flatten() else {
            return Err(Invalid("typing continuation needs a shared note"));
        };
        let provider = self
            .context
            .as_ref()
            .and_then(|context| context.analytic_continuation())
            .ok_or(Invalid("typing continuation provider is unavailable"))?;
        let id = self
            .context_id
            .ok_or(Invalid("typing continuation identity is unavailable"))?;
        let parameters = provider
            .parameters_at(id, &transition, absolute)
            .map_err(|_| Invalid("typing continuation boundary is invalid"))?;
        provider
            .sample_live_with_parameters(id, &parameters, &note, age, gate)
            .map_err(|_| Invalid("typing continuation sample was refused"))
    }
}
