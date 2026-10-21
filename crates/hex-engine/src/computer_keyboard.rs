//! Bounded, transient native keyboard audition. Preparation and destruction are control work.
use crate::{Frame, Invalid};
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    PreparedAnalyticNote, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
};
use hex_project::TrackId;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU64,
        Ordering::{Acquire, Release},
    },
};

pub const COMPUTER_KEY_COUNT: usize = 17;
pub const COMPUTER_KEY_VELOCITIES: [u8; 7] = [20, 40, 60, 80, 100, 120, 127];

/// The existing physical-key word law, shared by all scalar input boundaries.
pub fn valid_key_word(word: u64) -> bool {
    word == 0
        || ((1..=128).contains(&(word & 255))
            && word >> 16 != 0
            && COMPUTER_KEY_VELOCITIES.contains(&((word >> 8) as u8)))
}

/// Scalar desired-key snapshot for one output-bound native CURRENT activation.
/// It carries no source, runtime or admission authority.
#[derive(Clone, Copy, Debug)]
pub struct CurrentSamplerKeyboardFrame {
    pub epoch: u64,
    pub track: u64,
    pub device: u64,
    pub words: [u64; COMPUTER_KEY_COUNT],
}
const VOICES: usize = 64;
const NOTES: usize = 128 * COMPUTER_KEY_VELOCITIES.len();
const MAX_STORAGE: usize = 3 * 1024 * 1024;

/// One immutable activation lease. Cancellation bypasses every worker/audio queue.
/// Control is the sole desired-note writer; each physical key is independent.
/// Native sampler inputs remain control-only and carry an output/epoch pair;
/// UI lifecycle cancellation pairs `cancel` with the direct Client epoch revoke.
/// Create a fresh lease for every activation. Cancellation is terminal, including
/// after a successful graph replacement; held keys require release and a fresh press.
#[derive(Debug)]
pub struct ComputerKeyboardInput {
    cancelled: AtomicBool,
    notes: [AtomicU64; COMPUTER_KEY_COUNT],
    rejected: AtomicBool,
    native_activation: Option<(u64, u64)>,
}
impl Default for ComputerKeyboardInput {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notes: std::array::from_fn(|_| AtomicU64::new(0)),
            rejected: AtomicBool::new(false),
            native_activation: None,
        }
    }
}
impl ComputerKeyboardInput {
    pub fn with_native_sampler_activation(output: u64, epoch: u64) -> Result<Self, Invalid> {
        if output == 0 || epoch == 0 {
            return Err(Invalid("sampler keyboard activation identity is invalid"));
        }
        Ok(Self {
            native_activation: Some((output, epoch)),
            ..Self::default()
        })
    }
    pub fn native_sampler_activation(&self) -> Option<(u64, u64)> {
        self.native_activation
    }
    pub fn key_word(&self, index: usize) -> Option<u64> {
        self.notes.get(index).map(|n| n.load(Acquire))
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Acquire)
    }
    pub fn rejected(&self) -> bool {
        self.rejected.load(Acquire)
    }
    /// Token must be nonzero and monotonically unique for this activation.
    pub fn set_key(&self, key: usize, note: Option<(u8, u8, u64)>) -> Result<(), Invalid> {
        let slot = self
            .notes
            .get(key)
            .ok_or(Invalid("computer keyboard key is out of range"))?;
        let word = match note {
            None => 0,
            Some((pitch, velocity, token))
                if pitch <= 127
                    && COMPUTER_KEY_VELOCITIES.contains(&velocity)
                    && token > 0
                    && token <= (u64::MAX >> 16) =>
            {
                (token << 16) | (u64::from(velocity) << 8) | u64::from(pitch + 1)
            }
            Some(_) => return Err(Invalid("computer keyboard note is out of range")),
        };
        slot.store(word, Release);
        Ok(())
    }
}

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // One bounded selected-bank allocation; no per-note owners in the callback.
enum Sampler {
    Inline(PreparedAnalyticNote),
    Shared(PreparedSharedAnalyticNote),
}
#[derive(Clone, Copy)]
struct Voice {
    inline_override: Option<PreparedAnalyticNote>,
    key: usize,
    token: u64,
    sampler: usize,
    age: u64,
    gate: Option<u64>,
    id: u64,
}

pub(crate) enum KeyboardClock<'a> {
    Playing {
        timing: &'a dyn crate::MidiTiming,
        start: u64,
    },
    Monitor {
        beat: u64,
        elapsed: u64,
        micro_bpm: u32,
        sample_rate: u32,
    },
}
impl hex_lattice::NativeInstrumentClock for KeyboardClock<'_> {
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        match *self {
            Self::Playing { start, .. } => start.checked_add(u64::try_from(offset).ok()?),
            Self::Monitor { .. } => None,
        }
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(matches!(self, Self::Playing { .. }))
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        match *self {
            Self::Playing { timing, start } => timing
                .frame_to_beat(start.checked_add(offset as u64)?)
                .ok()
                .map(|beat| beat.raw()),
            Self::Monitor {
                beat,
                elapsed,
                micro_bpm,
                sample_rate,
            } => {
                let frames = elapsed.checked_add(offset as u64)?;
                let delta = u128::from(frames)
                    .checked_mul(u128::from(micro_bpm))?
                    .checked_mul(1_u128 << 32)?
                    / (u128::from(sample_rate) * 60_000_000);
                beat.checked_add(u64::try_from(delta).ok()?)
            }
        }
    }
}

/// Selected-track pool, owned by its ordinary prepared graph and retired with it.
/// Authored voices and live voices deliberately have independent mutable pools.
/// Control-only, lease-scoped continuation hint. Its private proof is obtained
/// from an engine-prepared playback; it cannot authorize another project family.
pub struct ComputerKeyboardTuningHint {
    family: crate::prepared_revision::TuningContinuationFamily,
    track: TrackId,
    input: Arc<ComputerKeyboardInput>,
    promoted: bool,
}

#[cfg(test)]
thread_local! {
    pub(crate) static CURRENT_SAMPLER_CONSTRUCTION_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
mod current_sampler;

/// Native CURRENT has no UI input owner. Its copied desired words arrive only
/// through the already-rooted output mailbox; ordinary playback keeps its lease.
enum KeyboardInput {
    Lease(Arc<ComputerKeyboardInput>),
    Current {
        words: [u64; COMPUTER_KEY_COUNT],
        cancelled: bool,
        rejected: bool,
    },
}
impl KeyboardInput {
    fn lease(&self) -> Option<&Arc<ComputerKeyboardInput>> {
        match self {
            Self::Lease(input) => Some(input),
            Self::Current { .. } => None,
        }
    }
    fn same_lease(&self, other: &Self) -> bool {
        match (self.lease(), other.lease()) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    fn cancel(&mut self) {
        match self {
            Self::Lease(input) => input.cancel(),
            Self::Current { cancelled, .. } => *cancelled = true,
        }
    }
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Lease(input) => input.is_cancelled(),
            Self::Current { cancelled, .. } => *cancelled,
        }
    }
    fn reject(&mut self) {
        match self {
            Self::Lease(input) => input.rejected.store(true, Release),
            Self::Current { rejected, .. } => *rejected = true,
        }
    }
    fn word(&self, index: usize) -> u64 {
        match self {
            Self::Lease(input) => input.notes[index].load(Acquire),
            Self::Current { words, .. } => words[index],
        }
    }
}

pub(crate) struct ComputerKeyboardRuntime {
    track: TrackId,
    instrument_id: Option<hex_project::DeviceId>,
    expression: Option<crate::live_expression::Snapshot>,
    input: KeyboardInput,
    samplers: Vec<Option<Sampler>>,
    supported: [bool; NOTES],
    context: Option<Box<dyn PreparedSharedAnalyticContext>>,
    mutable: Option<NativeInstrumentRuntimePair>,
    retarget: Option<Box<dyn hex_lattice::PreparedInstrumentHistoryRetarget>>,
    tuning_family: Option<crate::prepared_revision::TuningContinuationFamily>,
    tuning_preservation: bool,
    tuning: hex_lattice::TuningTable,
    release_frames: u64,
    primary: [Option<Voice>; VOICES],
    scratch: [Option<Voice>; VOICES],
    desired: [u64; COMPUTER_KEY_COUNT],
    next_id: u64,
    scratch_next_id: u64,
    age: u64,
    scratch_age: u64,
    monitor_anchor: Option<(u64, u64, u32)>,
    scratch_monitor_anchor: Option<(u64, u64, u32)>,
    boundary: bool,
    faulted: bool,
    preserve_on_handoff: bool,
    lease_detached: bool,
}
impl ComputerKeyboardRuntime {
    pub(crate) fn prepare(
        project: &hex_project::Project,
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
    ) -> Result<Self, Invalid> {
        let track_data = project
            .tracks()
            .iter()
            .find(|candidate| candidate.id() == track)
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        let device = track_data
            .instrument()
            .filter(|device| !device.bypassed())
            .ok_or(Invalid("select a track with an enabled instrument"))?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("instrument registry is unavailable"))?;
        let instrument = registry
            .prepare_instrument(
                device.state(),
                &hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: &project.tuning().table().map_err(|e| Invalid(e.0))?,
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .map_err(|_| Invalid("computer keyboard instrument preparation failed"))?;
        let mut runtime =
            Self::prepare_instrument(track, input, project.sample_rate(), instrument.as_ref())?;
        runtime.tuning = project.tuning().table().map_err(|e| Invalid(e.0))?;
        runtime.instrument_id = Some(device.id());
        Ok(runtime)
    }

    fn prepare_instrument(
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        sample_rate: u32,
        instrument: &dyn hex_lattice::PreparedNativeInstrument,
    ) -> Result<Self, Invalid> {
        Self::prepare_instrument_with_continuation(track, input, sample_rate, instrument, false)
    }
    fn prepare_instrument_with_continuation(
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        sample_rate: u32,
        instrument: &dyn hex_lattice::PreparedNativeInstrument,
        promoted: bool,
    ) -> Result<Self, Invalid> {
        let continuation = if promoted {
            instrument
                .prepare_mutable_continuation_runtime()
                .map_err(|_| Invalid("computer keyboard continuation preparation failed"))?
        } else {
            None
        };
        let factory = match continuation {
            Some(factory) => Some(factory),
            None => instrument
                .prepare_mutable_runtime()
                .map_err(|_| Invalid("computer keyboard runtime preparation failed"))?,
        };
        let retarget = factory
            .as_ref()
            .map(|factory| factory.prepare_history_retarget_target())
            .transpose()
            .map_err(|_| Invalid("computer keyboard retarget preparation failed"))?
            .and_then(|preparation| match preparation {
                hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) => Some(proof),
                _ => None,
            });
        let mutable = factory
            .as_ref()
            .map(|factory| {
                NativeInstrumentRuntimePair::new(
                    factory.as_ref(),
                    NativeInstrumentRuntimeLimits {
                        max_pair_bytes: 2 * 1024 * 1024,
                        max_events_per_block: 259,
                        max_active_occurrences: VOICES,
                        max_tail_frames: crate::mutable_instrument::max_mutable_tail_frames(
                            sample_rate,
                        )
                        .ok_or(hex_lattice::NativeInstrumentRuntimeError::RuntimeBudgetExceeded)?,
                    },
                )
            })
            .transpose()
            .map_err(|_| Invalid("computer keyboard runtime exceeds its storage budget"))?;
        let context = if mutable.is_none() {
            instrument
                .prepare_shared_analytic_context()
                .map_err(|_| Invalid("computer keyboard context preparation failed"))?
        } else {
            None
        };
        if context.as_ref().is_some_and(|context| {
            context.declared_context_bytes() > hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES
        }) {
            return Err(Invalid(
                "computer keyboard context exceeds its storage budget",
            ));
        }
        let mut samplers = Vec::new();
        if mutable.is_none() {
            samplers
                .try_reserve_exact(NOTES)
                .map_err(|_| Invalid("computer keyboard storage is unavailable"))?;
        }
        let mut supported = [false; NOTES];
        let mut release_frames = mutable
            .as_ref()
            .map_or(0, NativeInstrumentRuntimePair::max_release_tail_frames);
        for (velocity_index, velocity) in COMPUTER_KEY_VELOCITIES.into_iter().enumerate() {
            for pitch in 0..128_u8 {
                let index = velocity_index * 128 + usize::from(pitch);
                if let Some(runtime) = mutable.as_ref() {
                    // Validate against the actual native runtime too. A provider's
                    // optional source validator may not cover every pitch domain.
                    supported[index] = instrument.validate_event_note(pitch, velocity).is_ok()
                        && runtime
                            .preflight_event_batch(
                                &[NativeInstrumentEvent::NoteOn {
                                    frame_offset: 0,
                                    occurrence_id: 1,
                                    pitch,
                                    velocity,
                                }],
                                1,
                                &hex_lattice::EMPTY_DEVICE_RESOURCES,
                            )
                            .is_ok();
                    continue;
                }
                let sampler = if let Some(context) = &context {
                    context
                        .prepare_note(pitch, velocity)
                        .ok()
                        .map(Sampler::Shared)
                } else {
                    instrument
                        .prepare_note(pitch, velocity)
                        .ok()
                        .map(Sampler::Inline)
                };
                if let Some(sampler) = sampler {
                    release_frames = release_frames.max(match sampler {
                        Sampler::Inline(note) => note.release_frames(),
                        Sampler::Shared(note) => note.release_frames(),
                    });
                    supported[index] = true;
                }
                samplers.push(sampler);
            }
        }
        if !supported.iter().any(|supported| *supported) {
            return Err(Invalid(
                "computer keyboard instrument supports no playable pitches",
            ));
        }
        let result = Self {
            track,
            instrument_id: None,
            expression: None,
            input: KeyboardInput::Lease(input),
            samplers,
            supported,
            context,
            mutable,
            retarget,
            tuning_family: None,
            tuning_preservation: false,
            tuning: Default::default(),
            release_frames,
            primary: [None; VOICES],
            scratch: [None; VOICES],
            desired: [0; COMPUTER_KEY_COUNT],
            next_id: 0,
            scratch_next_id: 0,
            age: 0,
            scratch_age: 0,
            monitor_anchor: None,
            scratch_monitor_anchor: None,
            boundary: false,
            faulted: false,
            preserve_on_handoff: false,
            lease_detached: false,
        };
        if result
            .storage_bytes()
            .is_none_or(|bytes| bytes > MAX_STORAGE)
        {
            return Err(Invalid("computer keyboard storage exceeds its budget"));
        }
        Ok(result)
    }
    pub(crate) fn set_tuning_family(
        &mut self,
        family: crate::prepared_revision::TuningContinuationFamily,
    ) {
        self.tuning_family = Some(family);
    }
    pub(crate) fn tuning_hint(&self) -> Option<ComputerKeyboardTuningHint> {
        Some(ComputerKeyboardTuningHint {
            family: self.tuning_family?,
            track: self.track,
            input: Arc::clone(self.input.lease()?),
            promoted: self.mutable.is_some(),
        })
    }
    pub(crate) fn prepare_for_tuning(
        project: &hex_project::Project,
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        family: crate::prepared_revision::TuningContinuationFamily,
        hint: &ComputerKeyboardTuningHint,
    ) -> Result<Self, Invalid> {
        if hint.family != family
            || hint.track != track
            || !Arc::ptr_eq(&hint.input, &input)
            || input.is_cancelled()
        {
            return Err(Invalid("computer keyboard tuning lease is stale"));
        }
        let device = project
            .track(track)
            .and_then(|track| track.instrument())
            .filter(|device| !device.bypassed())
            .ok_or(Invalid("computer keyboard instrument is unavailable"))?;
        let tuning = project.tuning().table().map_err(|e| Invalid(e.0))?;
        let registry = hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("instrument registry is unavailable"))?;
        let instrument = registry
            .prepare_instrument(
                device.state(),
                &hex_lattice::DevicePreparationContext {
                    sample_rate: project.sample_rate(),
                    tuning: &tuning,
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .map_err(|_| Invalid("computer keyboard tuning preparation failed"))?;
        let mut result = Self::prepare_instrument_with_continuation(
            track,
            input,
            project.sample_rate(),
            instrument.as_ref(),
            hint.promoted,
        )?;
        result.instrument_id = Some(device.id());
        result.tuning = tuning;
        result.tuning_family = Some(family);
        result.tuning_preservation = true;
        result.preserve_on_handoff = true;
        Ok(result)
    }
    pub(crate) fn tuning_preservation_matches(&self, previous: &Self) -> bool {
        self.tuning_preservation
            && self.tuning_family.is_some()
            && self.tuning_family == previous.tuning_family
            && self.track == previous.track
            && self.instrument_id == previous.instrument_id
            && self.input.same_lease(&previous.input)
    }

    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.retarget
                    .as_ref()
                    .map_or(0, |proof| proof.declared_fixed_storage_bytes()),
            )?
            .checked_add(if self.input.lease().is_some() {
                std::mem::size_of::<ComputerKeyboardInput>() + 2 * std::mem::size_of::<usize>()
            } else {
                0
            })?
            .checked_add(
                self.samplers
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<Sampler>>())?,
            )?
            .checked_add(
                self.context
                    .as_ref()
                    .map_or(0, |context| context.declared_context_bytes()),
            )?
            .checked_add(self.mutable.as_ref().map_or(
                Some(0),
                NativeInstrumentRuntimePair::fixed_storage_pair_bytes,
            )?)
    }
    pub(crate) fn musical_clock<'a>(
        &mut self,
        timing: &'a crate::prepared_tempo::SessionTimingView<'_>,
        performance_frame: u64,
        playing: bool,
    ) -> Result<KeyboardClock<'a>, Invalid> {
        if playing {
            self.scratch_monitor_anchor = None;
            return Ok(KeyboardClock::Playing {
                timing,
                start: performance_frame,
            });
        }
        let (start_age, beat, micro_bpm) = match self.scratch_monitor_anchor {
            Some(anchor) => anchor,
            None => {
                use crate::MidiTiming;
                let anchor = (
                    self.scratch_age,
                    timing
                        .frame_to_beat(performance_frame)
                        .map_err(|_| Invalid("computer keyboard monitor clock unavailable"))?
                        .raw(),
                    timing
                        .micro_bpm_at_frame(performance_frame)
                        .map_err(|_| Invalid("computer keyboard monitor tempo unavailable"))?,
                );
                self.scratch_monitor_anchor = Some(anchor);
                anchor
            }
        };
        use crate::MidiTiming;
        Ok(KeyboardClock::Monitor {
            beat,
            elapsed: self
                .scratch_age
                .checked_sub(start_age)
                .ok_or(Invalid("computer keyboard monitor clock reversed"))?,
            micro_bpm,
            sample_rate: timing.sample_rate(),
        })
    }
    pub(crate) fn request_controller_handoff_preservation(&mut self) {
        self.preserve_on_handoff = true;
    }
    fn preserves_input_identity(&self, previous: &Self) -> bool {
        self.preserve_on_handoff && self.input.same_lease(&previous.input)
    }
    pub(crate) fn requests_preservation_from(&self, previous: &Self) -> bool {
        self.preserves_input_identity(previous) && !self.input.is_cancelled()
    }
    fn continued_voices(&self, previous: &Self) -> Result<[Option<Voice>; VOICES], Invalid> {
        let mut voices = previous.primary;
        for voice in voices.iter_mut().flatten() {
            if !self.supported.get(voice.sampler).copied().unwrap_or(false) {
                return Err(Invalid(
                    "held computer keyboard pitch is unsupported by tuning",
                ));
            }
            if self.mutable.is_none() && self.tuning != previous.tuning {
                let target = self.samplers.get(voice.sampler).copied().flatten();
                let source = voice
                    .inline_override
                    .map(Sampler::Inline)
                    .or_else(|| previous.samplers.get(voice.sampler).copied().flatten());
                match (target, source) {
                    (Some(Sampler::Inline(target)), Some(Sampler::Inline(source))) => {
                        voice.inline_override =
                            Some(target.continue_from(&source, voice.age).map_err(|_| {
                                Invalid("computer keyboard pitch continuation was refused")
                            })?);
                    }
                    _ => {
                        return Err(Invalid(
                            "computer keyboard pitch continuation is unsupported",
                        ));
                    }
                }
            }
        }
        Ok(voices)
    }
    fn analytic_seeds(
        previous: &Self,
    ) -> Option<([hex_lattice::NativeInstrumentAnalyticSeed; VOICES], usize)> {
        let mut notes = [hex_lattice::NativeInstrumentAnalyticSeed::default(); VOICES];
        let mut len = 0;
        for voice in previous.primary.iter().flatten() {
            let Some(Sampler::Shared(note)) =
                previous.samplers.get(voice.sampler).copied().flatten()
            else {
                return None;
            };
            notes[len] = hex_lattice::NativeInstrumentAnalyticSeed {
                occurrence_id: voice.id,
                note,
                age_frames: voice.age,
                gate_frame: voice.gate,
            };
            len += 1;
        }
        Some((notes, len))
    }
    pub(crate) fn can_preserve_from(&self, previous: &Self) -> bool {
        // The requests_preservation_from cut decides whether the input is live.
        // Structural preflight and commit must not re-read asynchronous cancel.
        if !self.preserves_input_identity(previous)
            || previous.lease_detached
            || previous.faulted
            || self.track != previous.track
            || self.instrument_id != previous.instrument_id
            || self.release_frames != previous.release_frames
            || self.continued_voices(previous).is_err()
        {
            return false;
        }
        match (&self.mutable, &previous.mutable) {
            (Some(next), Some(old)) => {
                next.can_inherit_from(old)
                    || (self.tuning_preservation_matches(previous)
                        && self
                            .retarget
                            .as_ref()
                            .is_some_and(|proof| next.can_retarget_from(old, proof.as_ref())))
            }
            (None, None) => self.samplers.len() == previous.samplers.len(),
            (Some(next), None) if self.tuning_preservation_matches(previous) => {
                let Some(context) = previous.context.as_deref() else {
                    return false;
                };
                let Some((notes, len)) = Self::analytic_seeds(previous) else {
                    return false;
                };
                let Ok(source_id) = hex_lattice::AnalyticContinuationId::new(1) else {
                    return false;
                };
                next.can_import_analytic_history(&hex_lattice::AnalyticInstrumentHistory {
                    source: context,
                    source_id,
                    transition: None,
                    performance_frame: previous.age,
                    voices: &notes[..len],
                })
            }
            _ => false,
        }
    }
    /// Every provider and both twins are checked before the input lease changes.
    pub(crate) fn preserve_from(&mut self, previous: &mut Self) -> Result<(), Invalid> {
        // Playback already preflighted before moving any Session state. A
        // control-side cancel can arrive after that cut: carry its terminal
        // input state, rather than reject a partially transferred graph. All
        // history/identity fields remain audio-owned and are rechecked here.
        if !self.can_preserve_from(previous) {
            return Err(Invalid("computer keyboard continuation was refused"));
        }
        let voices = self.continued_voices(previous)?;
        match (&mut self.mutable, &previous.mutable) {
            (Some(next), Some(old)) => {
                if next.can_inherit_from(old) {
                    next.inherit_from(old)
                } else {
                    let proof = self
                        .retarget
                        .as_deref()
                        .ok_or(Invalid("computer keyboard retarget proof is missing"))?;
                    next.retarget_from(old, proof)
                }
                .map_err(|_| Invalid("computer keyboard mutable history was refused"))?;
            }
            (Some(next), None) => {
                let context = previous
                    .context
                    .as_deref()
                    .ok_or(Invalid("computer keyboard source context is missing"))?;
                let (notes, len) = Self::analytic_seeds(previous)
                    .ok_or(Invalid("computer keyboard source notes are unsupported"))?;
                let source_id = hex_lattice::AnalyticContinuationId::new(1)
                    .map_err(|_| Invalid("computer keyboard source identity is invalid"))?;
                next.import_analytic_history(&hex_lattice::AnalyticInstrumentHistory {
                    source: context,
                    source_id,
                    transition: None,
                    performance_frame: previous.age,
                    voices: &notes[..len],
                })
                .map_err(|_| Invalid("computer keyboard analytic history was refused"))?;
            }
            _ => {}
        }
        self.primary = voices;
        self.scratch = voices;
        self.desired = previous.desired;
        self.next_id = previous.next_id;
        self.scratch_next_id = previous.next_id;
        self.age = previous.age;
        self.scratch_age = previous.age;
        self.monitor_anchor = previous.monitor_anchor;
        self.scratch_monitor_anchor = previous.monitor_anchor;
        self.boundary = previous.boundary;
        self.expression = previous.expression;
        self.preserve_on_handoff = false;
        previous.lease_detached = true;
        previous.cancel();
        Ok(())
    }
    pub(crate) fn set_expression(
        &mut self,
        selected: Option<(
            TrackId,
            hex_project::DeviceId,
            crate::live_expression::Snapshot,
        )>,
    ) {
        self.expression = match selected {
            Some((track, instrument, snapshot))
                if track == self.track && self.instrument_id == Some(instrument) =>
            {
                Some(snapshot)
            }
            _ => self
                .expression
                .map(|_| crate::live_expression::Snapshot::default()),
        };
    }
    pub(crate) fn track(&self) -> TrackId {
        self.track
    }
    pub(crate) fn cancel(&mut self) {
        if !self.lease_detached {
            self.input.cancel();
        }
        self.primary.fill(None);
        self.scratch.fill(None);
        if let Some(runtime) = &mut self.mutable {
            runtime.reset();
        }
    }
    pub(crate) fn begin_block(&mut self) {
        if self.lease_detached || self.input.is_cancelled() || self.faulted {
            self.primary.fill(None);
            if let Some(runtime) = &mut self.mutable {
                runtime.reset();
            }
        }
        self.scratch = self.primary;
        self.scratch_next_id = self.next_id;
        self.scratch_age = self.age;
        self.scratch_monitor_anchor = self.monitor_anchor;
        self.desired = std::array::from_fn(|index| {
            if self.lease_detached || self.input.is_cancelled() || self.faulted {
                0
            } else {
                self.input.word(index)
            }
        });
        self.boundary = true;
        if let Some(runtime) = &mut self.mutable {
            runtime.begin_block();
        }
    }
    pub(crate) fn rollback(&mut self) {
        if let Some(runtime) = &mut self.mutable {
            runtime.rollback_block();
        }
    }
    pub(crate) fn commit(&mut self) {
        self.primary = self.scratch;
        self.next_id = self.scratch_next_id;
        self.age = self.scratch_age;
        self.monitor_anchor = self.scratch_monitor_anchor;
        if let Some(runtime) = &mut self.mutable {
            runtime.commit_block();
        }
    }
    pub(crate) fn render(
        &mut self,
        output: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), Invalid> {
        self.render_with_resources(output, clock, &hex_lattice::EMPTY_DEVICE_RESOURCES)
    }
    pub(crate) fn render_with_resources(
        &mut self,
        output: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), Invalid> {
        if output.is_empty() {
            return Ok(());
        }
        let mut events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 0,
        }; 2 * VOICES + 3];
        let mut len = 0;
        if let Some(expression) = self.expression {
            events[..3].copy_from_slice(&expression.events());
            len = 3;
        }
        self.scratch.iter_mut().for_each(|slot| {
            if slot.is_some_and(|voice| {
                voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(self.release_frames))
            }) {
                *slot = None;
            }
        });
        if self.boundary {
            for voice in self.scratch.iter_mut().flatten() {
                if voice.gate.is_none() && self.desired[voice.key] != voice.token {
                    voice.gate = Some(voice.age);
                    events[len] = NativeInstrumentEvent::NoteOff {
                        frame_offset: 0,
                        occurrence_id: voice.id,
                    };
                    len += 1;
                }
            }
            for (key, token) in self.desired.into_iter().enumerate() {
                if token == 0
                    || self
                        .scratch
                        .iter()
                        .flatten()
                        .any(|voice| voice.key == key && voice.token == token)
                {
                    continue;
                }
                let pitch = ((token & 255) as u8).saturating_sub(1);
                let velocity = ((token >> 8) & 255) as u8;
                let velocity_index = COMPUTER_KEY_VELOCITIES
                    .iter()
                    .position(|value| *value == velocity)
                    .ok_or(Invalid("computer keyboard velocity is invalid"))?;
                let sampler = velocity_index * 128 + usize::from(pitch);
                if !self.supported[sampler] {
                    self.input.reject();
                    continue;
                }
                let capacity = self
                    .mutable
                    .as_ref()
                    .map_or(VOICES, NativeInstrumentRuntimePair::max_active_occurrences);
                let event_capacity = self.mutable.as_ref().map_or(
                    2 * VOICES,
                    NativeInstrumentRuntimePair::max_events_per_block,
                );
                if self.scratch.iter().flatten().count() >= capacity || len >= event_capacity {
                    self.input.reject();
                    self.faulted = true;
                    self.input.cancel();
                    self.scratch.fill(None);
                    if let Some(runtime) = &mut self.mutable {
                        runtime.reset();
                        runtime.begin_block();
                    }
                    len = 0;
                    break;
                }
                let Some(slot) = self.scratch.iter_mut().find(|slot| slot.is_none()) else {
                    self.input.reject();
                    self.faulted = true;
                    self.scratch.fill(None);
                    if let Some(runtime) = &mut self.mutable {
                        runtime.reset();
                        runtime.begin_block();
                    }
                    len = 0;
                    break;
                };
                self.scratch_next_id = self
                    .scratch_next_id
                    .checked_add(1)
                    .ok_or(Invalid("computer keyboard identity exhausted"))?;
                *slot = Some(Voice {
                    inline_override: None,
                    key,
                    token,
                    sampler,
                    age: 0,
                    gate: None,
                    id: self.scratch_next_id,
                });
                events[len] = NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: self.scratch_next_id,
                    pitch,
                    velocity,
                };
                len += 1;
            }
            self.boundary = false;
        }
        if let Some(runtime) = &mut self.mutable {
            runtime
                .process_block_with_clock(&events[..len], output, clock, resources)
                .map_err(|_| Invalid("computer keyboard runtime refused a note block"))?;
        } else {
            for voice in self.scratch.iter().flatten() {
                let Some(sampler) = voice
                    .inline_override
                    .map(Sampler::Inline)
                    .or(self.samplers[voice.sampler])
                else {
                    continue;
                };
                for (offset, target) in output.iter_mut().enumerate() {
                    let age = voice
                        .age
                        .checked_add(offset as u64)
                        .ok_or(Invalid("computer keyboard age overflow"))?;
                    let gate = voice.gate.unwrap_or(u64::MAX);
                    let sample = match sampler {
                        Sampler::Inline(note) => {
                            note.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                        }
                        Sampler::Shared(note) => self
                            .context
                            .as_ref()
                            .ok_or(Invalid("computer keyboard context missing"))?
                            .sample(&note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                    };
                    target[0] += sample[0];
                    target[1] += sample[1];
                }
            }
        }
        self.scratch_age = self
            .scratch_age
            .checked_add(output.len() as u64)
            .ok_or(Invalid("computer keyboard clock overflow"))?;
        for voice in self.scratch.iter_mut().flatten() {
            voice.age = voice
                .age
                .checked_add(output.len() as u64)
                .ok_or(Invalid("computer keyboard age overflow"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PreparedRevision,
        playback::{Playback, PlaybackCommand},
    };
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

    pub(super) fn fixture() -> (Project, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        (editor.project().clone(), track)
    }
    fn prepared(project: &Project) -> Arc<PreparedRevision> {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    }
    fn playback(project: &Project, track: TrackId, input: &Arc<ComputerKeyboardInput>) -> Playback {
        let mut playback = Playback::from_prepared(prepared(project));
        playback
            .prepare_computer_keyboard(project, track, Arc::clone(input))
            .unwrap();
        playback
    }
    #[test]
    fn stopped_keyboard_pairs_cover_real_device_input_output_and_freeze_clocks() {
        let (project, track) = fixture();
        let mut editor = Editor::new(project, 16).unwrap();
        let state =
            crate::default_native_device_state(crate::DeviceTypeId::from_static("hex.utility"))
                .unwrap()
                .with_input_gain_millidb(6_000)
                .unwrap()
                .with_output_gain_millidb(-3_000)
                .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut tested = playback(editor.project(), track, &input);
        let mut oracle = playback(editor.project(), track, &input);
        let mut actual = [[0.0; 2]; 512];
        let mut expected = actual;
        let mut calls = Vec::new();
        tested
            .render_with_node_signal_observer(
                &mut actual,
                Some(&mut |identity, phase, offset, frames: &[Frame]| {
                    assert_eq!(
                        identity.owner,
                        hex_lattice::ProcessorChainOwner::Track(track.value())
                    );
                    calls.push((phase, offset, frames.to_vec()));
                }),
            )
            .unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.001));
        assert_eq!(calls.len(), 4);
        for (chunk, pair) in calls.chunks_exact(2).enumerate() {
            assert_eq!(
                (pair[0].0, pair[0].1),
                (crate::NodeSignalPhase::Input, chunk * 256)
            );
            assert_eq!(
                (pair[1].0, pair[1].1),
                (crate::NodeSignalPhase::Output, chunk * 256)
            );
            assert_eq!(pair[1].2, actual[chunk * 256..(chunk + 1) * 256]);
            assert_ne!(pair[0].2, pair[1].2);
        }
        assert_eq!((tested.position(), tested.performance_frame()), (0, 0));
        assert!(!tested.playing());
    }
    #[test]
    fn computer_keyboard_stopped_polyphony_release_matches_independent_sine_pcm_and_freezes_transport()
     {
        let (project, track) = fixture();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let a = instrument.prepare_note(60, 100).unwrap();
        let b = instrument.prepare_note(64, 80).unwrap();
        for partition in [1, 17, 127, 256] {
            let input = Arc::new(ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 100, 1))).unwrap();
            input.set_key(1, Some((64, 80, 2))).unwrap();
            let mut playback = playback(&project, track, &input);
            let mut output = [[0.0; 2]; 512];
            for chunk in output.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            for (age, actual) in output.into_iter().enumerate() {
                let x = a.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let y = b.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                assert_eq!(actual, [x[0] + y[0], x[1] + y[1]]);
            }
            input.set_key(0, None).unwrap();
            input.set_key(1, None).unwrap();
            let mut release = [[0.0; 2]; 3000];
            for chunk in release.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            for (offset, actual) in release.into_iter().enumerate() {
                let x = a.sample(
                    512 + offset as u64,
                    512,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
                let y = b.sample(
                    512 + offset as u64,
                    512,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
                assert_eq!(actual, [x[0] + y[0], x[1] + y[1]]);
            }
            assert!(!playback.playing());
            assert_eq!(playback.position(), 0);
            assert_eq!(playback.performance_frame(), 0);
        }
    }
    #[test]
    fn controller_only_keyboard_publication_preserves_held_pcm_and_detaches_retired_lease() {
        let (project, track) = fixture();
        let revision = prepared(&project);
        let input = Arc::new(ComputerKeyboardInput::default());
        let oracle_input = Arc::new(ComputerKeyboardInput::default());
        for keys in [&input, &oracle_input] {
            keys.set_key(0, Some((60, 100, 1))).unwrap();
        }
        let mut old = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(Arc::clone(&revision));
        old.prepare_computer_keyboard(&project, track, Arc::clone(&input))
            .unwrap();
        oracle
            .prepare_computer_keyboard(&project, track, Arc::clone(&oracle_input))
            .unwrap();
        let mut actual = [[0.0; 2]; 257];
        let mut expected = actual;
        old.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        let mut next = Playback::from_prepared(revision);
        next.prepare_computer_keyboard_for_controller_activation(
            &project,
            track,
            Arc::clone(&input),
        )
        .unwrap();
        next.try_inherit_runtime_from(&mut old).unwrap();
        assert!(!input.is_cancelled());
        old.cancel_computer_keyboard();
        old.apply(PlaybackCommand::Stop);
        drop(old);
        assert!(
            !input.is_cancelled(),
            "retirement cannot cancel the adopted physical-key lease"
        );
        next.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "held notes keep their committed phase, envelope and age"
        );
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        input.set_key(0, None).unwrap();
        oracle_input.set_key(0, None).unwrap();
        next.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "the original physical key still sends its note off"
        );
    }

    #[test]
    fn controller_keyboard_preservation_rejects_different_revision_and_never_revives_cancelled_input()
     {
        let (project, track) = fixture();
        let revision = prepared(&project);
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut old = Playback::from_prepared(Arc::clone(&revision));
        old.prepare_computer_keyboard(&project, track, Arc::clone(&input))
            .unwrap();
        old.render(&mut [[0.0; 2]; 64]).unwrap();
        let mut wrong = Playback::from_prepared(prepared(&project));
        wrong
            .prepare_computer_keyboard_for_controller_activation(
                &project,
                track,
                Arc::clone(&input),
            )
            .unwrap();
        assert!(wrong.try_inherit_runtime_from(&mut old).is_err());
        assert!(
            !input.is_cancelled(),
            "rejection leaves the active owner untouched"
        );
        input.cancel();
        let mut cancelled = Playback::from_prepared(revision);
        cancelled
            .prepare_computer_keyboard_for_controller_activation(
                &project,
                track,
                Arc::clone(&input),
            )
            .unwrap();
        cancelled.try_inherit_runtime_from(&mut old).unwrap();
        let mut silent = [[1.0; 2]; 64];
        cancelled.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 64]);
        assert!(input.is_cancelled());
    }

    #[test]
    fn computer_keyboard_cancel_stop_and_bank_replacement_never_retrigger_old_held_keys() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut old = playback(&project, track, &input);
        old.apply(PlaybackCommand::Play);
        old.render(&mut [[0.0; 2]; 256]).unwrap();
        assert!(!input.is_cancelled());
        let mut new = Playback::from_prepared(prepared(&project));
        new.try_inherit_runtime_from(&mut old).unwrap();
        assert!(
            input.is_cancelled(),
            "successful same-revision handoff must end the old lease"
        );
        let mut silent = [[1.0; 2]; 256];
        new.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 256]);
        let fresh = Arc::new(ComputerKeyboardInput::default());
        fresh.set_key(0, Some((60, 100, 1))).unwrap();
        new.prepare_computer_keyboard(&project, track, Arc::clone(&fresh))
            .unwrap();
        new.render(&mut silent).unwrap();
        assert!(silent.iter().flatten().any(|sample| sample.abs() > 1e-6));
        new.apply(PlaybackCommand::Stop);
        assert!(fresh.is_cancelled());
        new.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 256]);
        assert_eq!(new.position(), 0);
    }
    #[test]
    fn computer_keyboard_voice_pressure_cancels_only_live_pool_and_does_not_stick() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let mut playback = playback(&project, track, &input);
        for token in 1..=80 {
            input.set_key(0, Some((60, 100, token))).unwrap();
            playback.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        assert!(input.rejected());
        let mut output = [[1.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 256]);
    }
    #[test]
    fn computer_keyboard_source_binding_refuses_mismatched_instrument_and_rate() {
        let (project, track) = fixture();
        let mut playback = Playback::from_prepared(prepared(&project));
        let mut snapshot = project.snapshot();
        snapshot.sample_rate = 44_100;
        assert!(
            playback
                .prepare_computer_keyboard(
                    &Project::from_snapshot(snapshot).unwrap(),
                    track,
                    Arc::new(ComputerKeyboardInput::default())
                )
                .is_err()
        );
        let mut editor = Editor::new(project, 4).unwrap();
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        assert!(
            playback
                .prepare_computer_keyboard(
                    editor.project(),
                    track,
                    Arc::new(ComputerKeyboardInput::default())
                )
                .is_err()
        );
    }
    #[test]
    fn computer_keyboard_invalid_payloads_and_cancelled_late_preparation_are_silent() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        assert!(input.set_key(17, Some((60, 100, 1))).is_err());
        assert!(input.set_key(0, Some((60, 99, 1))).is_err());
        assert!(input.set_key(0, Some((60, 100, 0))).is_err());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.cancel();
        let mut playback = playback(&project, track, &input);
        let mut output = [[1.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 256]);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{
        PreparedRevision,
        playback::{Playback, PlaybackCommand},
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::collections::BTreeMap;
    fn prepare(project: &Project) -> Arc<PreparedRevision> {
        Arc::new(
            PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    }
    #[test]
    fn computer_keyboard_graph_preparation_preserves_sounding_authored_note_history() {
        let (source, track) = super::tests::fixture();
        let mut editor = Editor::new(source, 16).unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 48,
                velocity: 80,
                channel: 0,
            })
            .unwrap();
        let source = editor.project();
        let revision = prepare(source);
        let mut old = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(Arc::clone(&revision));
        for runtime in [&mut old, &mut oracle] {
            runtime.apply(PlaybackCommand::Play);
            runtime.render(&mut [[0.0; 2]; 257]).unwrap();
        }
        let mut candidate = Playback::from_prepared(revision);
        let input = Arc::new(ComputerKeyboardInput::default());
        candidate
            .prepare_computer_keyboard(source, track, Arc::clone(&input))
            .unwrap();
        candidate.try_inherit_runtime_from(&mut old).unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut with_live = [[0.0; 2]; 256];
        let mut authored = with_live;
        candidate.render(&mut with_live).unwrap();
        oracle.render(&mut authored).unwrap();
        assert_ne!(with_live, authored);
        input.cancel();
        candidate.render(&mut with_live).unwrap();
        oracle.render(&mut authored).unwrap();
        assert_eq!(
            with_live, authored,
            "live cancellation must preserve the authored sounding note"
        );
        assert_eq!(
            (candidate.position(), candidate.performance_frame()),
            (769, 769)
        );
    }
    #[test]
    fn computer_keyboard_fader_applies_once_to_live_and_authored_notes() {
        let (project, track) = super::tests::fixture();
        let mut editor = Editor::new(project, 16).unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 48,
                velocity: 80,
                channel: 0,
            })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].gain = 0.25;
        let project = Project::from_snapshot(snapshot).unwrap();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48000).unwrap();
        let live = instrument.prepare_note(60, 100).unwrap();
        let authored = instrument.prepare_note(48, 80).unwrap();
        for playing in [false, true] {
            let input = Arc::new(ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 100, 1))).unwrap();
            let mut runtime = Playback::from_prepared(prepare(&project));
            runtime
                .prepare_computer_keyboard(&project, track, input)
                .unwrap();
            if playing {
                runtime.apply(PlaybackCommand::Play);
            }
            let mut output = [[0.0; 2]; 512];
            runtime.render(&mut output).unwrap();
            for (age, actual) in output.iter().enumerate() {
                let sample =
                    (live.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES)[0]
                        + if playing {
                            authored.sample(age as u64, 48000, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                                [0]
                        } else {
                            0.0
                        })
                        * 0.25;
                assert!(
                    (actual[0] - sample).abs() < 1e-6,
                    "playing={playing} age={age} actual={actual:?} expected={sample}"
                );
                assert!((actual[1] - sample).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn computer_keyboard_silent_session_route_still_renders_live_notes() {
        let (project, track) = super::tests::fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut runtime = Playback::from_prepared(prepare(&project));
        runtime
            .prepare_computer_keyboard(&project, track, input)
            .unwrap();
        let mut groups = 0;
        assert_eq!(
            runtime.visit_candidate_groups(&mut |_, _, _| {
                groups += 1;
                Ok(())
            }),
            Err(crate::AudioStorageError::UnsupportedGraph)
        );
        assert_eq!(
            groups, 5,
            "only outer Playback locations may precede the live-input refusal"
        );
        runtime.apply(PlaybackCommand::Play);
        runtime
            .try_apply_action(crate::PlaybackAction::Session(
                crate::SessionPlaybackCommand::StopTrack {
                    track,
                    quantization: crate::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let mut output = [[0.0; 2]; 256];
        runtime.render(&mut output).unwrap();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48000).unwrap();
        let note = instrument.prepare_note(60, 100).unwrap();
        for (age, actual) in output.iter().enumerate() {
            assert_eq!(
                *actual,
                note.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            );
        }
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
    }
    #[test]
    fn computer_keyboard_stopped_live_audio_follows_typed_routing_and_one_suffix() {
        use hex_devices::utility::{UTILITY_DEFINITION, parameter};
        let (source, track) = super::tests::fixture();
        let mut editor = Editor::new(source, 16).unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::INVERT_LEFT,
                ParameterValue::Bool(true),
            )
            .unwrap();
        for _ in 0..2 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].instrument_position = Some(1);
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        let Created::Track(destination) = editor
            .execute(Command::AddTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("bus")
        };
        editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(track),
                destination,
                gain: 0.5,
            })
            .unwrap();
        let source = editor.project();
        let mut playback = Playback::from_prepared(prepare(source));
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(source, track, input)
            .unwrap();
        let mut output = [[0.0; 2]; 512];
        playback.render(&mut output).unwrap();
        let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
        let note = instrument.prepare_note(60, 100).unwrap();
        for (age, actual) in output.into_iter().enumerate() {
            let sample = note.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES)[0];
            assert!(
                (actual[0] + sample * 1.5).abs() < 1e-6,
                "left at {age}: {actual:?} vs {sample}"
            );
            assert!((actual[1] - sample * 1.5).abs() < 1e-6);
        }
        assert_eq!((playback.position(), playback.performance_frame()), (0, 0));
    }
    #[test]
    fn computer_keyboard_mutable_operator_has_polyphony_and_bounded_release() {
        let (source, track) = super::tests::fixture();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let mut state = metadata.default_state().unwrap();
        state = metadata
            .patch_parameter(
                &state,
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
        let mut editor = Editor::new(source, 8).unwrap();
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let source = editor.project();
        let mut playback = Playback::from_prepared(prepare(source));
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        playback
            .prepare_computer_keyboard(source, track, Arc::clone(&input))
            .unwrap();
        let mut output = [[0.0; 2]; 256];
        playback.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        input.set_key(0, None).unwrap();
        input.set_key(1, None).unwrap();
        for _ in 0..2000 {
            playback.render(&mut output).unwrap();
        }
        assert_eq!(output, [[0.0; 2]; 256]);
        assert!(!input.rejected());
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use crate::{
        MidiTiming, PreparedTempoMap,
        prepared_tempo::{
            AnchoredSessionTempoView, SessionBaseTimingView, SessionClockAnchor, SessionTimingView,
        },
    };
    use hex_lattice::NativeInstrumentClock;
    use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};
    #[test]
    fn computer_keyboard_musical_clock_uses_nonzero_performance_and_anchors_monitor_at_current_tempo()
     {
        let map = PreparedTempoMap::prepare(
            TempoMap::new(
                48_000,
                vec![
                    TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
                    TempoPoint::new(
                        BeatTime::from_quarters(1).unwrap(),
                        Tempo::new(60_000_000).unwrap(),
                    ),
                    TempoPoint::new(
                        BeatTime::from_quarters(5).unwrap(),
                        Tempo::new(30_000_000).unwrap(),
                    ),
                ],
            )
            .unwrap(),
            48_000,
        )
        .unwrap();
        let performance = map
            .beat_to_frame(BeatTime::from_quarters(4).unwrap())
            .unwrap();
        let anchor = SessionClockAnchor {
            cut_frame: performance,
            old_beat: BeatTime::from_quarters(2).unwrap(),
            new_beat: BeatTime::from_quarters(4).unwrap(),
        };
        let timing = SessionTimingView::Anchored(AnchoredSessionTempoView::new(
            SessionBaseTimingView::Canonical(&map),
            anchor,
        ));
        let (project, track) = super::tests::fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let mut runtime = ComputerKeyboardRuntime::prepare(&project, track, input).unwrap();
        runtime.begin_block();
        let clock = runtime.musical_clock(&timing, performance, true).unwrap();
        assert_eq!(clock.transport_is_running(), Some(true));
        assert_eq!(
            clock.performance_frame_at_offset(48_000),
            Some(performance + 48_000)
        );
        assert_eq!(
            clock.beat_at_offset(0),
            Some(BeatTime::from_quarters(2).unwrap().raw())
        );
        assert_eq!(
            clock.beat_at_offset(48_000),
            timing
                .frame_to_beat(performance + 48_000)
                .ok()
                .map(|beat| beat.raw())
        );
        let clock = runtime.musical_clock(&timing, performance, false).unwrap();
        assert_eq!(clock.transport_is_running(), Some(false));
        assert_eq!(clock.performance_frame_at_offset(0), None);
        assert_eq!(clock.performance_frame_at_offset(96_000), None);
        assert_eq!(
            clock.beat_at_offset(96_000),
            Some(BeatTime::from_quarters(4).unwrap().raw()),
            "stopped monitor stays at60bpm beyond future30bpm marker"
        );
        runtime.scratch_age = 48_000;
        let clock = runtime.musical_clock(&timing, performance, false).unwrap();
        assert_eq!(
            clock.beat_at_offset(0),
            Some(BeatTime::from_quarters(3).unwrap().raw())
        );
        runtime.rollback();
        runtime.begin_block();
        assert!(
            runtime.scratch_monitor_anchor.is_none(),
            "failed monitor block must not retain clock anchor"
        );
    }
}

#[cfg(test)]
mod pitch_domain_tests {
    use super::*;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    #[test]
    fn computer_keyboard_and_authored_track_admit_existing_sixty_second_operator_release() {
        let (source, track) = super::tests::fixture();
        let mut editor = hex_project::Editor::new(source, 4).unwrap();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let mut state = metadata.default_state().unwrap();
        for (id, value) in [(38, 60.0), (78, 0.2)] {
            state = metadata
                .patch_parameter(
                    &state,
                    ParameterId::from_static(id),
                    ParameterValue::Float(value),
                )
                .unwrap();
        }
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(hex_project::Command::ReplaceDeviceState { device, state })
            .unwrap();
        let hex_project::Created::Clip(clip) = editor
            .execute(hex_project::Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(hex_project::Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
        let input = Arc::new(ComputerKeyboardInput::default());
        let keyboard = ComputerKeyboardRuntime::prepare(editor.project(), track, input).unwrap();
        assert!(keyboard.release_frames >= 48_000 * 60);
        assert!(keyboard.mutable.is_some());
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = crate::SessionPlayback::new(revision).unwrap();
        playback.apply(crate::SessionPlaybackCommand::Play).unwrap();
        let mut pcm = [[0.0; 2]; 128];
        playback.render(&mut pcm).unwrap();
        assert!(pcm.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    }

    #[test]
    fn computer_keyboard_mutable_nyquist_refusal_keeps_audio_render_usable() {
        let (source, track) = super::tests::fixture();
        let mut snapshot = source.snapshot();
        snapshot.sample_rate = 8000;
        let mut source =
            hex_project::Editor::new(hex_project::Project::from_snapshot(snapshot).unwrap(), 4)
                .unwrap();
        let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
        let state = metadata
            .patch_parameter(
                &metadata.default_state().unwrap(),
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
        let device = source
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        source
            .execute(hex_project::Command::ReplaceDeviceState { device, state })
            .unwrap();
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                source.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = crate::playback::Playback::from_prepared(revision);
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((127, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(source.project(), track, Arc::clone(&input))
            .unwrap();
        let mut frames = [[1.0; 2]; 256];
        playback.render(&mut frames).unwrap();
        assert!(input.rejected());
        assert_eq!(frames, [[0.0; 2]; 256]);
        input.set_key(0, Some((60, 100, 2))).unwrap();
        playback.render(&mut frames).unwrap();
        assert!(frames.iter().flatten().any(|sample| sample.abs() > 1e-6));
    }
}

#[cfg(test)]
#[path = "computer_keyboard_clock_admission_tests.rs"]
mod clock_admission_tests;

#[cfg(test)]
#[path = "computer_keyboard/tuning_tests.rs"]
mod tuning_tests;
