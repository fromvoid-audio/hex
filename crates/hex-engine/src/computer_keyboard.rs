//! Bounded, transient native keyboard audition. Preparation and destruction are control work.
use crate::mutable_instrument::MutableInstrumentTrackRuntime;
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
const COMPUTER_NOTES: usize = 128 * COMPUTER_KEY_VELOCITIES.len();
const NOTES: usize = 128 * 127;
/// Existing selected-keyboard runtime ceiling, including its bounded mutable pair/context.
/// Control consumers reserve this before attachment and still validate actual graph storage.
pub const MAX_COMPUTER_KEYBOARD_STORAGE_BYTES: usize = 3 * 1024 * 1024;

mod activation;
mod capture;
mod live_notes;
mod live_runtime;
pub(crate) mod ordered_keys;
mod source_input;
pub(crate) use live_notes::LiveNoteBatch;
pub use live_notes::{LiveNoteActivation, LiveNoteEvent, LiveNoteInput, LiveNotePublishError};
pub use ordered_keys::MidiKeyActivation;
/// Full exact-velocity hardware banks are prepared only for opted-in roots.
pub const MAX_LIVE_KEYBOARD_STORAGE_BYTES: usize = 9 * 1024 * 1024;
pub(crate) mod note_audition;
pub use activation::KeyboardActivationClock;
use note_audition::{AuditionGate, AuditionRequest};
pub use note_audition::{MAX_NOTE_AUDITION_GATE_FRAMES, MAX_NOTE_AUDITION_NOTES};

/// One transient input root with terminal lifecycle cancellation. An ordinary output-bound
/// root may reserve fresh scalar epochs after priority Stop; default/unbound inputs and the
/// legacy native sampler activation retain terminal Stop semantics. Control is the sole
/// desired-note writer; no source, runtime or admission authority lives here.
#[derive(Debug)]
pub struct ComputerKeyboardInput {
    cancelled: AtomicBool,
    notes: [AtomicU64; COMPUTER_KEY_COUNT],
    rejected: AtomicBool,
    native_activation: Option<(u64, u64)>,
    output_activation: Option<activation::OutputActivation>,
    sequence: AtomicU64,
    audition: Option<note_audition::NoteAudition>,
    live_notes: Option<LiveNoteInput>,
    live_audition: Option<note_audition::LiveAudition>,
    audition_started: AtomicU64,
    audition_finished: AtomicU64,
    live_output_epoch: AtomicU64,
    pub(crate) ordered_keys: Option<Box<ordered_keys::OrderedKeys>>,
}
impl Default for ComputerKeyboardInput {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notes: std::array::from_fn(|_| AtomicU64::new(0)),
            rejected: AtomicBool::new(false),
            native_activation: None,
            output_activation: None,
            sequence: AtomicU64::new(0),
            audition: None,
            live_notes: None,
            live_audition: None,
            audition_started: AtomicU64::new(0),
            audition_finished: AtomicU64::new(0),
            live_output_epoch: AtomicU64::new(0),
            ordered_keys: None,
        }
    }
}
impl ComputerKeyboardInput {
    /// Opt a fresh ordinary input into ordered hardware notes before admission.
    /// Native input opens only after this exact root's preparation succeeds.
    pub fn with_live_notes(mut self) -> Result<Self, Invalid> {
        if self.native_activation.is_some()
            || self.audition.is_some()
            || self.live_notes.is_some()
            || self.sequence.load(Acquire) != 0
            || self.is_cancelled()
        {
            return Err(Invalid("live notes require a fresh ordinary input"));
        }
        self.live_output_epoch.store(
            self.output_activation
                .as_ref()
                .map_or(0, |a| a.epoch.load(Acquire)),
            Release,
        );
        self.live_notes = Some(LiveNoteInput::default());
        self.live_audition = Some(note_audition::LiveAudition::default());
        Ok(self)
    }
    /// Reconnect one hardware activation on control, retaining physical voices.
    /// After Stop, reserve a fresh output epoch before opening native input.
    pub fn begin_live_notes_activation_on_control(&self) -> Result<LiveNoteActivation, Invalid> {
        if self.is_cancelled() {
            return Err(Invalid("live note input is retired"));
        }
        if self.output_activation.is_some() {
            self.ensure_output_activation_on_control()?;
        }
        let input = self
            .live_notes
            .as_ref()
            .ok_or(Invalid("live note input is unavailable"))?;
        let activation = input
            .begin_activation_on_control()
            .map_err(|_| Invalid("live note activation is busy or unavailable"))?;
        self.live_output_epoch.store(
            self.output_activation
                .as_ref()
                .map_or(0, |a| a.epoch.load(Acquire)),
            Release,
        );
        Ok(activation)
    }
    pub fn live_notes(&self) -> Option<&LiveNoteInput> {
        self.live_notes.as_ref()
    }
    pub fn runtime_storage_limit_on_control(&self) -> usize {
        if self.live_notes.is_some() {
            MAX_LIVE_KEYBOARD_STORAGE_BYTES
        } else {
            MAX_COMPUTER_KEYBOARD_STORAGE_BYTES
        }
    }

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
        if let Some(input) = &self.live_notes {
            input.cancel();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Acquire)
    }
    pub fn rejected(&self) -> bool {
        self.rejected.load(Acquire)
    }
    /// Token must be nonzero and monotonically unique for this activation.
    pub fn set_key(&self, key: usize, note: Option<(u8, u8, u64)>) -> Result<(), Invalid> {
        if self.is_note_audition() {
            return Err(Invalid("note audition must be triggered as one chord"));
        }
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
        let after = self.begin_write()?;
        if word != 0 {
            self.cancel_live_note_audition_on_control();
        }
        let before = slot.swap(word, std::sync::atomic::Ordering::SeqCst);
        if let Some(queue) = &self.ordered_keys {
            queue.publish_change(key, before, word);
        }
        self.sequence
            .store(after, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // One bounded selected-bank allocation; no per-note owners in the callback.
enum Sampler {
    Inline(PreparedAnalyticNote),
    Shared(PreparedSharedAnalyticNote),
}
// Keep the ordinary/CURRENT physical support map compact. Only an explicitly
// admitted hardware root owns the larger exact-velocity table on control.
#[allow(clippy::large_enum_variant)]
enum SupportedNotes {
    Computer([bool; COMPUTER_NOTES]),
    Live(Box<[bool; NOTES]>),
}
impl std::ops::Deref for SupportedNotes {
    type Target = [bool];
    fn deref(&self) -> &[bool] {
        match self {
            Self::Computer(values) => values,
            Self::Live(values) => values.as_slice(),
        }
    }
}
impl std::ops::DerefMut for SupportedNotes {
    fn deref_mut(&mut self) -> &mut [bool] {
        match self {
            Self::Computer(values) => values,
            Self::Live(values) => values.as_mut_slice(),
        }
    }
}
impl SupportedNotes {
    fn extra_storage_bytes(&self) -> usize {
        if matches!(self, Self::Live(_)) {
            NOTES
        } else {
            0
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum VoiceSource {
    Physical,
    Audition,
    Hardware { channel: u8, pitch: u8 },
}
#[derive(Clone, Copy)]
struct Voice {
    source: VoiceSource,
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
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        match *self {
            Self::Playing { timing, start } => {
                timing.tempo_micro_bpm_at_frame(start.checked_add(offset as u64)?)
            }
            Self::Monitor { micro_bpm, .. } => Some(micro_bpm),
        }
    }
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
pub(crate) enum NoteSnapshot {
    Active(u64, [u64; COMPUTER_KEY_COUNT], Option<AuditionRequest>),
    Inactive,
    Deferred(u64),
}
pub(crate) enum KeyboardInput {
    Lease(Arc<ComputerKeyboardInput>),
    Current {
        words: [u64; COMPUTER_KEY_COUNT],
        cancelled: bool,
        rejected: bool,
    },
}
impl KeyboardInput {
    pub(crate) fn lease(&self) -> Option<&Arc<ComputerKeyboardInput>> {
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
    fn snapshot(&self) -> NoteSnapshot {
        match self {
            Self::Lease(input) => input.note_snapshot(),
            Self::Current {
                words, cancelled, ..
            } => {
                if *cancelled {
                    NoteSnapshot::Inactive
                } else {
                    NoteSnapshot::Active(0, *words, None)
                }
            }
        }
    }
}

pub(crate) struct ComputerKeyboardRuntime {
    track: TrackId,
    instrument_id: Option<hex_project::DeviceId>,
    expression: Option<crate::live_expression::Snapshot>,
    pub(crate) input: KeyboardInput,
    samplers: Vec<Option<Sampler>>,
    supported: SupportedNotes,
    context: Option<Box<dyn PreparedSharedAnalyticContext>>,
    mutable: Option<NativeInstrumentRuntimePair>,
    source_input: Option<Box<source_input::SourceInputState>>,
    pub(crate) resources: Option<crate::native_device_resources::PreparedDeviceResourceLease>,
    retarget: Option<Box<dyn hex_lattice::PreparedInstrumentHistoryRetarget>>,
    tuning_family: Option<crate::prepared_revision::TuningContinuationFamily>,
    tuning_preservation: bool,
    tuning: hex_lattice::TuningTable,
    release_frames: u64,
    primary: [Option<Voice>; VOICES],
    scratch: [Option<Voice>; VOICES],
    desired: [u64; COMPUTER_KEY_COUNT],
    rejected_desired: [u64; COMPUTER_KEY_COUNT],
    scratch_rejected_desired: [u64; COMPUTER_KEY_COUNT],
    activation_epoch: Option<u64>,
    next_id: u64,
    scratch_next_id: u64,
    age: u64,
    scratch_age: u64,
    audition_gate: AuditionGate,
    scratch_audition_gate: AuditionGate,
    monitor_anchor: Option<(u64, u64, u32)>,
    scratch_monitor_anchor: Option<(u64, u64, u32)>,
    boundary: bool,
    faulted: bool,
    preserve_on_handoff: bool,
    lease_detached: bool,
    live_batch: Option<live_notes::LiveNoteBatch>,
    live_consumed: usize,
    live_activation: Option<LiveNoteActivation>,
    scratch_live_activation: Option<LiveNoteActivation>,
    pub(crate) capture: Option<Box<crate::midi_capture::CaptureStorage>>,
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
        Self::prepare_from_capabilities(
            track,
            input,
            sample_rate,
            Some(instrument),
            factory.as_deref(),
            None,
            None,
        )
    }
    pub(crate) fn prepare_resource_mutable(
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        sample_rate: u32,
        factory: &dyn hex_lattice::PreparedMutableInstrumentRuntime,
        resources: crate::native_device_resources::PreparedDeviceResourceLease,
    ) -> Result<Self, Invalid> {
        Self::prepare_from_capabilities(
            track,
            input,
            sample_rate,
            None,
            Some(factory),
            Some(resources),
            None,
        )
    }
    fn prepare_from_capabilities(
        track: TrackId,
        input: Arc<ComputerKeyboardInput>,
        sample_rate: u32,
        instrument: Option<&dyn hex_lattice::PreparedNativeInstrument>,
        factory: Option<&dyn hex_lattice::PreparedMutableInstrumentRuntime>,
        resources: Option<crate::native_device_resources::PreparedDeviceResourceLease>,
        source: Option<&source_input::SourceInputPreparation<'_>>,
    ) -> Result<Self, Invalid> {
        let resource_view = resources.as_ref().map_or(
            &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
            |lease| lease.resources(),
        );
        let retarget = factory
            .map(|factory| factory.prepare_history_retarget_target())
            .transpose()
            .map_err(|_| Invalid("computer keyboard retarget preparation failed"))?
            .and_then(|preparation| match preparation {
                hex_lattice::InstrumentHistoryRetargetPreparation::Prepared(proof) => Some(proof),
                _ => None,
            });
        let mutable = factory
            .map(|factory| {
                NativeInstrumentRuntimePair::new(
                    factory,
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
        let context = if mutable.is_none() && source.is_none() {
            instrument
                .ok_or(Invalid(
                    "computer keyboard analytic capability is unavailable",
                ))?
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
        if mutable.is_none() && source.and_then(|source| source.runtime).is_none() {
            samplers
                .try_reserve_exact(input.template_count())
                .map_err(|_| Invalid("computer keyboard storage is unavailable"))?;
        }
        let mut supported = if input.live_notes.is_some() {
            SupportedNotes::Live(Box::new([false; NOTES]))
        } else {
            SupportedNotes::Computer([false; COMPUTER_NOTES])
        };
        let mut release_frames = source.and_then(|source| source.runtime).map_or_else(
            || {
                mutable
                    .as_ref()
                    .map_or(0, NativeInstrumentRuntimePair::max_release_tail_frames)
            },
            MutableInstrumentTrackRuntime::max_release_tail_frames,
        );
        for (index, supported_note) in supported
            .iter_mut()
            .enumerate()
            .take(input.template_count())
        {
            let (pitch, velocity) = input
                .template_note(index)
                .ok_or(Invalid("keyboard template index is invalid"))?;
            if let Some(runtime) = source.and_then(|source| source.runtime) {
                *supported_note = runtime.validates_live_note(pitch, velocity);
                continue;
            }
            if let Some(runtime) = mutable.as_ref() {
                // Validate against the actual native runtime too. A provider's
                // optional source validator may not cover every pitch domain.
                *supported_note = instrument.is_none_or(|instrument| {
                    instrument.validate_event_note(pitch, velocity).is_ok()
                }) && runtime
                    .preflight_event_batch(
                        &[NativeInstrumentEvent::NoteOn {
                            frame_offset: 0,
                            occurrence_id: 1,
                            pitch,
                            velocity,
                        }],
                        1,
                        resource_view,
                    )
                    .is_ok();
                continue;
            }
            let sampler = if let Some(owner) = source.and_then(|source| source.context.as_ref()) {
                owner
                    .context()
                    .prepare_note(pitch, velocity)
                    .ok()
                    .map(Sampler::Shared)
            } else if let Some(context) = &context {
                context
                    .prepare_note(pitch, velocity)
                    .ok()
                    .map(Sampler::Shared)
            } else {
                instrument
                    .ok_or(Invalid(
                        "computer keyboard analytic capability is unavailable",
                    ))?
                    .prepare_note(pitch, velocity)
                    .ok()
                    .map(Sampler::Inline)
            };
            if let Some(sampler) = sampler {
                release_frames = release_frames.max(match sampler {
                    Sampler::Inline(note) => note.release_frames(),
                    Sampler::Shared(note) => note.release_frames(),
                });
                *supported_note = true;
            }
            samplers.push(sampler);
        }
        if input.is_note_audition() {
            if supported[..input.template_count()]
                .iter()
                .any(|valid| !valid)
            {
                return Err(Invalid(
                    "instrument cannot audition the complete selected chord",
                ));
            }
            if let Some(runtime) = &mutable {
                if input.template_count() > runtime.max_active_occurrences() {
                    return Err(Invalid("selected chord exceeds instrument voice capacity"));
                }
                let mut events = [NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 0,
                }; COMPUTER_KEY_COUNT];
                for (index, event) in events.iter_mut().enumerate().take(input.template_count()) {
                    let (pitch, velocity) = input
                        .template_note(index)
                        .ok_or(Invalid("keyboard template index is invalid"))?;
                    *event = NativeInstrumentEvent::NoteOn {
                        frame_offset: 0,
                        occurrence_id: index as u64 + 1,
                        pitch,
                        velocity,
                    };
                }
                runtime
                    .preflight_event_batch(&events[..input.template_count()], 1, resource_view)
                    .map_err(|_| Invalid("instrument refused the complete selected chord"))?;
            }
        }
        if !supported.iter().any(|supported| *supported) {
            return Err(Invalid(
                "computer keyboard instrument supports no playable pitches",
            ));
        }
        let storage_limit = input.runtime_storage_limit_on_control();
        let capture = input
            .ordered_keys
            .as_ref()
            .map(|_| Box::new(crate::midi_capture::CaptureStorage::default()));
        let result = Self {
            resources,
            track,
            instrument_id: None,
            expression: None,
            input: KeyboardInput::Lease(input),
            samplers,
            supported,
            context,
            mutable,
            source_input: source
                .map(|source| source_input::SourceInputState::prepare(track, source))
                .transpose()?,
            retarget,
            tuning_family: None,
            tuning_preservation: false,
            tuning: Default::default(),
            release_frames,
            primary: [None; VOICES],
            scratch: [None; VOICES],
            desired: [0; COMPUTER_KEY_COUNT],
            rejected_desired: [0; COMPUTER_KEY_COUNT],
            scratch_rejected_desired: [0; COMPUTER_KEY_COUNT],
            activation_epoch: None,
            next_id: 0,
            scratch_next_id: 0,
            age: 0,
            scratch_age: 0,
            audition_gate: AuditionGate::default(),
            scratch_audition_gate: AuditionGate::default(),
            monitor_anchor: None,
            scratch_monitor_anchor: None,
            boundary: false,
            faulted: false,
            preserve_on_handoff: false,
            lease_detached: false,
            live_batch: None,
            live_consumed: 0,
            live_activation: None,
            scratch_live_activation: None,
            capture,
        };
        if result
            .storage_bytes()
            .is_none_or(|bytes| bytes > storage_limit)
        {
            return Err(Invalid("computer keyboard storage exceeds its budget"));
        }
        Ok(result)
    }
    pub(crate) fn set_resource_identity(
        &mut self,
        device: hex_project::DeviceId,
        tuning: hex_lattice::TuningTable,
    ) {
        self.instrument_id = Some(device);
        self.tuning = tuning;
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
            promoted: self.mutable.is_some()
                || self
                    .source_input
                    .as_ref()
                    .is_some_and(|source| source.mutable),
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
                self.source_input
                    .as_ref()
                    .map_or(0, |_| std::mem::size_of::<source_input::SourceInputState>()),
            )?
            .checked_add(self.supported.extra_storage_bytes())?
            .checked_add(self.capture.as_ref().map_or(0, |_| {
                std::mem::size_of::<crate::midi_capture::CaptureStorage>()
            }))?
            .checked_add(
                self.retarget
                    .as_ref()
                    .map_or(0, |proof| proof.declared_fixed_storage_bytes()),
            )?
            .checked_add(
                self.input
                    .lease()
                    .map_or(0, |input| input.control_storage_bytes()),
            )?
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
        (self.preserve_on_handoff
            || (self.source_input.is_some()
                && previous.source_input.is_some()
                && self.track == previous.track
                && self.instrument_id == previous.instrument_id))
            && self.input.same_lease(&previous.input)
    }
    pub(crate) fn requests_preservation_from(&self, previous: &Self) -> bool {
        self.preserves_input_identity(previous) && !self.input.is_cancelled()
    }
    // Input custody is distinct from voice history. A freshly constructed ordinary monitor
    // may read the same physical keys against a different source; its old voices never move.
    pub(crate) fn can_rebind_input_from(&self, previous: &Self) -> bool {
        self.track == previous.track
            && self.input.same_lease(&previous.input)
            && !self.input.is_cancelled()
            && !self.lease_detached
            && !self.faulted
            && self.age == 0
            && self.primary.iter().all(Option::is_none)
            && self.scratch.iter().all(Option::is_none)
            && !previous.lease_detached
            && !previous.faulted
    }
    pub(crate) fn detach_rebound_input(&mut self) {
        self.lease_detached = true;
    }
    fn continued_voices(&self, previous: &Self) -> Result<[Option<Voice>; VOICES], Invalid> {
        let mut voices = previous.primary;
        for voice in voices.iter_mut().flatten() {
            if !self.supported.get(voice.sampler).copied().unwrap_or(false) {
                return Err(Invalid(
                    "held computer keyboard pitch is unsupported by tuning",
                ));
            }
            if self.mutable.is_none()
                && self.tuning != previous.tuning
                && !self
                    .source_input
                    .as_ref()
                    .is_some_and(|source| source.mutable)
            {
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
        let resources_match = match (&self.resources, &previous.resources) {
            (None, None) => true,
            (Some(current), Some(previous)) => current.compatible(previous),
            _ => false,
        };
        if !resources_match || self.source_input.is_some() != previous.source_input.is_some() {
            return false;
        }

        // The requests_preservation_from cut decides whether the input is live.
        // Structural preflight and commit must not re-read asynchronous cancel.
        if !self.preserves_input_identity(previous)
            || previous.lease_detached
            || previous.faulted
            || self.track != previous.track
            || self.instrument_id != previous.instrument_id
            || (self.release_frames != previous.release_frames && self.source_input.is_none())
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
            (None, None) => {
                self.source_input.is_some() || self.samplers.len() == previous.samplers.len()
            }
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
        if let Some((next, old)) = self
            .source_input
            .as_mut()
            .zip(previous.source_input.as_ref())
        {
            next.id = old.id;
            next.release = old.release;
            next.scratch_release = old.release;
        }
        self.primary = voices;
        self.scratch = voices;
        self.desired = previous.desired;
        self.rejected_desired = previous.rejected_desired;
        self.scratch_rejected_desired = previous.rejected_desired;
        self.live_activation = previous.live_activation;
        self.scratch_live_activation = previous.live_activation;
        self.activation_epoch = previous.activation_epoch;
        if let Some((next, old)) = self.capture.as_mut().zip(previous.capture.as_ref()) {
            next.next_keys = old.next_keys;
            next.scratch_next_keys = old.next_keys;
        }
        self.next_id = previous.next_id;
        self.scratch_next_id = previous.next_id;
        self.age = previous.age;
        self.scratch_age = previous.age;
        self.audition_gate = previous.audition_gate;
        self.scratch_audition_gate = previous.audition_gate;
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
        if let Some(source) = &mut self.source_input {
            source.release = true;
        }
        self.fail_capture(crate::midi_capture::MidiCaptureFault::InputChanged);
        if !self.lease_detached {
            self.input.cancel();
        }
        self.primary.fill(None);
        self.scratch.fill(None);
        if let Some(runtime) = &mut self.mutable {
            runtime.reset();
        }
    }
    fn reset_voices(&mut self) {
        if let Some(source) = &mut self.source_input {
            source.release = true;
        }
        let source_next_id = self
            .source_input
            .as_ref()
            .map(|_| self.next_id.max(self.scratch_next_id));
        self.primary.fill(None);
        self.scratch.fill(None);
        self.live_activation = None;
        self.scratch_live_activation = None;
        self.desired.fill(0);
        self.rejected_desired.fill(0);
        self.scratch_rejected_desired.fill(0);
        self.next_id = source_next_id.unwrap_or(0);
        self.scratch_next_id = self.next_id;
        self.age = 0;
        self.scratch_age = 0;
        self.audition_gate = AuditionGate::default();
        self.scratch_audition_gate = AuditionGate::default();
        self.monitor_anchor = None;
        self.scratch_monitor_anchor = None;
        if let Some(runtime) = &mut self.mutable {
            runtime.reset();
        }
    }
    /// Callback priority Stop retains only an ordinary output-bound root. Its output clock
    /// already revoked the old prefix on control; never revoke newer physical input here.
    pub(crate) fn priority_stop(&mut self, clock: &KeyboardActivationClock) {
        self.fail_capture(crate::midi_capture::MidiCaptureFault::InputChanged);
        if !self.lease_detached
            && self
                .input
                .lease()
                .is_some_and(|i| i.bound_to_clock(clock) && !i.is_note_audition())
        {
            self.reset_voices();
            self.activation_epoch = None;
        } else {
            self.cancel();
        }
    }
    pub(crate) fn begin_block(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.key_batch = self
                .input
                .lease()
                .and_then(|input| input.ordered_keys.as_ref())
                .filter(|_| !self.lease_detached && !self.faulted)
                .and_then(|queue| queue.snapshot());
            capture.keys_consumed = 0;
            capture.scratch_next_keys = capture.next_keys;
        }
        self.live_batch = self
            .input
            .lease()
            .and_then(|input| input.live_notes.as_ref())
            .filter(|_| !self.lease_detached && !self.faulted)
            .and_then(|input| input.snapshot().ok());
        self.live_consumed = 0;
        let snapshot = if self.lease_detached || self.faulted {
            NoteSnapshot::Inactive
        } else {
            self.input.snapshot()
        };
        let deferred = matches!(&snapshot, NoteSnapshot::Deferred(_));
        match snapshot {
            NoteSnapshot::Active(epoch, words, audition) => {
                if self.activation_epoch != Some(epoch) {
                    self.reset_voices();
                }
                self.activation_epoch = Some(epoch);
                self.desired = words;
                self.audition_gate
                    .observe(audition, self.input.lease().map(Arc::as_ref));
                if let Some(baseline) = self
                    .input
                    .lease()
                    .and_then(|input| input.ordered_keys.as_ref())
                    .and_then(|queue| queue.baseline())
                {
                    for (word, before) in self.desired.iter_mut().zip(baseline) {
                        if *word != before {
                            *word = 0;
                        }
                    }
                }
            }
            NoteSnapshot::Inactive => {
                self.fail_capture(crate::midi_capture::MidiCaptureFault::InputChanged);
                self.reset_voices();
                self.activation_epoch = None;
            }
            NoteSnapshot::Deferred(epoch) => {
                if self.activation_epoch != Some(epoch) {
                    self.reset_voices();
                    self.activation_epoch = None;
                }
            }
        }
        if let Some(capture) = &mut self.capture
            && capture
                .key_batch
                .as_ref()
                .is_some_and(|batch| Some(batch.output_epoch) != self.activation_epoch)
        {
            if !deferred {
                if let Some((queue, batch)) = self
                    .input
                    .lease()
                    .and_then(|input| input.ordered_keys.as_ref())
                    .zip(capture.key_batch.as_ref())
                {
                    queue.fault(batch.activation);
                }
                if capture.active {
                    capture.fail(crate::midi_capture::MidiCaptureFault::InputChanged);
                }
            }
            capture.key_batch = None;
        }
        if let Some(capture) = self.capture.as_mut().filter(|capture| capture.is_active())
            && let Some(binding) = capture.binding
        {
            let key_changed = self
                .input
                .lease()
                .and_then(|input| input.ordered_keys.as_ref())
                .is_none_or(|queue| !queue.active(binding.keys))
                || capture
                    .key_batch
                    .as_ref()
                    .is_some_and(|batch| batch.activation != binding.keys);
            let hardware_changed = match binding.hardware {
                Some(token) => self
                    .live_batch
                    .as_ref()
                    .is_none_or(|batch| batch.activation != token),
                None => self.live_batch.is_some(),
            };
            if key_changed || hardware_changed {
                let overflow = self.input.lease().is_some_and(|input| {
                    !input.is_cancelled()
                        && (input.ordered_keys_faulted(binding.keys)
                            || binding.hardware.is_some_and(|token| {
                                input
                                    .live_notes()
                                    .is_some_and(|live| live.activation_rejected(token))
                            }))
                });
                capture.fail(if overflow {
                    crate::midi_capture::MidiCaptureFault::Capacity
                } else {
                    crate::midi_capture::MidiCaptureFault::InputChanged
                });
            }
        }
        // Deferred writes cannot keep a finite gate open.
        if self.audition_gate.expired() {
            self.desired.fill(0);
        }
        self.scratch_live_activation = self.live_activation;
        self.scratch_rejected_desired = self.rejected_desired;
        self.scratch = self.primary;
        self.scratch_next_id = self.next_id;
        self.scratch_age = self.age;
        self.scratch_audition_gate = self.audition_gate;
        self.scratch_monitor_anchor = self.monitor_anchor;
        self.boundary = true;
        if let Some(source) = &mut self.source_input {
            source.scratch_release = source.release;
            source.len = 0;
        }
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
        if let Some(source) = &mut self.source_input {
            source.release = false;
        }
        if let Some(capture) = &mut self.capture {
            if let Some((queue, batch)) = self
                .input
                .lease()
                .and_then(|input| input.ordered_keys.as_ref())
                .zip(capture.key_batch.as_ref())
            {
                queue.commit(batch, capture.keys_consumed);
            }
            capture.next_keys = capture.scratch_next_keys;
        }
        if let Some((input, batch)) = self
            .input
            .lease()
            .and_then(|input| input.live_notes.as_ref())
            .zip(self.live_batch.as_ref())
        {
            let _ = input.commit(batch, self.live_consumed);
        }
        self.live_activation = self.scratch_live_activation;
        self.rejected_desired = self.scratch_rejected_desired;
        self.primary = self.scratch;
        self.next_id = self.scratch_next_id;
        self.age = self.scratch_age;
        self.audition_gate = self.scratch_audition_gate;
        if let Some(input) = self.input.lease() {
            self.audition_gate.commit(input);
        }
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
        if self
            .input
            .lease()
            .is_some_and(|input| input.live_notes.is_some() || input.ordered_keys.is_some())
        {
            self.render_live_spans(output, clock, resources)
        } else {
            self.render_voice_span(output, clock, resources, None, None)
        }
    }
    fn render_voice_span(
        &mut self,
        output: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        resources: &dyn hex_lattice::DeviceResources,
        live_event: Option<LiveNoteEvent>,
        key_event: Option<ordered_keys::KeyEvent>,
    ) -> Result<(), Invalid> {
        let before_voices = self
            .capture
            .as_ref()
            .filter(|capture| capture.is_active())
            .map(|_| self.scratch);
        // Live-note handlers may update the pool before borrowing a resource view.
        self.scratch.iter_mut().for_each(|slot| {
            if slot.is_some_and(|voice| {
                voice
                    .gate
                    .is_some_and(|gate| voice.age >= gate.saturating_add(self.release_frames))
            }) {
                *slot = None;
            }
        });
        let mut events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 0,
        }; 259];
        let mut len = 0;
        if let Some(activation) = self.live_batch.as_ref().map(|batch| batch.activation)
            && self.scratch_live_activation != Some(activation)
        {
            self.release_hardware(None, &mut events, &mut len);
            self.scratch_live_activation = Some(activation);
        }
        if let Some(event) = live_event {
            self.apply_live_event(event, &mut events, &mut len)?;
        }
        if let Some(event) = key_event {
            self.apply_key_event(event, &mut events, &mut len)?;
        }
        if output.is_empty() {
            return Ok(());
        }
        if let Some(expression) = self
            .expression
            .filter(|_| self.boundary && self.source_input.is_none())
        {
            events[len..len + 3].copy_from_slice(&expression.events());
            len += 3;
        }
        if self.boundary
            && self
                .capture
                .as_ref()
                .is_none_or(|capture| capture.key_batch.is_none())
        {
            let desired_source = if self.scratch_audition_gate.active() {
                VoiceSource::Audition
            } else {
                VoiceSource::Physical
            };
            for voice in self.scratch.iter_mut().flatten() {
                if matches!(voice.source, VoiceSource::Physical | VoiceSource::Audition)
                    && voice.gate.is_none()
                    && (voice.source != desired_source || self.desired[voice.key] != voice.token)
                {
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
                    || self.scratch_rejected_desired[key] == token
                    || self.scratch.iter().flatten().any(|voice| {
                        voice.source == desired_source && voice.key == key && voice.token == token
                    })
                {
                    continue;
                }
                let pitch = ((token & 255) as u8).saturating_sub(1);
                let velocity = ((token >> 8) & 255) as u8;
                let sampler = self.input.sampler_index(key, pitch, velocity)?;
                let shared_audition = desired_source == VoiceSource::Audition
                    && self
                        .input
                        .lease()
                        .is_some_and(|input| input.live_notes.is_some());
                if !self.supported[sampler] {
                    if !shared_audition {
                        self.input.reject();
                    }
                    continue;
                }
                let capacity = self.voice_capacity();
                let event_capacity = self.mutable.as_ref().map_or(
                    2 * VOICES,
                    NativeInstrumentRuntimePair::max_events_per_block,
                );
                if self.scratch.iter().flatten().count() >= capacity || len >= event_capacity {
                    if shared_audition {
                        self.scratch_audition_gate.finish();
                        break;
                    }
                    self.input.reject();
                    if self.source_input.is_some()
                        || self.input.lease().is_some_and(|input| {
                            input.live_notes.is_some() || input.ordered_keys.is_some()
                        })
                    {
                        // Refuse this physical press once without cancelling hardware
                        // or replaying the held token later when a tail expires.
                        self.scratch_rejected_desired[key] = token;
                        continue;
                    }
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
                    source: desired_source,
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
        }
        self.scratch_audition_gate.advance(output.len() as u64);
        self.boundary = false;
        if let Some(before) = before_voices {
            self.capture_voice_changes(&before, clock);
        }
        let resources = self
            .resources
            .as_ref()
            .map_or(resources, |lease| lease.resources());
        if let Some(source) = self.source_input.as_mut().filter(|source| source.mutable) {
            if len > source.events.len() {
                return Err(Invalid("source live event capacity exceeded"));
            }
            source.events[..len].copy_from_slice(&events[..len]);
            source.len = len;
        } else if let Some(runtime) = &mut self.mutable {
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
                        Sampler::Shared(note) => {
                            let context = self
                                .source_input
                                .as_ref()
                                .and_then(|source| source.context.as_ref())
                                .map(|owner| owner.context())
                                .or(self.context.as_deref())
                                .ok_or(Invalid("computer keyboard context missing"))?;
                            context.sample(&note, age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                        }
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
    pub(super) fn prepared(project: &Project) -> Arc<PreparedRevision> {
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
    pub(super) fn playback(
        project: &Project,
        track: TrackId,
        input: &Arc<ComputerKeyboardInput>,
    ) -> Playback {
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
    fn output_keyboard_priority_stop_revokes_old_prefix_and_preserves_new_physical_epoch() {
        let (project, track) = fixture();
        let clock = Arc::new(KeyboardActivationClock::default());
        let input =
            Arc::new(ComputerKeyboardInput::with_output_activation(Arc::clone(&clock)).unwrap());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut actual = playback(&project, track, &input);
        let mut output = [[0.0; 2]; 64];
        actual.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        clock.revoke_reserved_prefix();
        actual.apply_priority_stop(&clock);
        actual.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 64]);
        assert!(!input.is_cancelled());

        // Reserve after Stop on control, then consume that older Stop on the callback.
        input.reactivate_on_control().unwrap();
        input.set_key(0, Some((60, 100, 2))).unwrap();
        actual.apply_priority_stop(&clock);
        actual.render(&mut output).unwrap();
        let oracle_input = Arc::new(ComputerKeyboardInput::default());
        oracle_input.set_key(0, Some((60, 100, 2))).unwrap();
        let mut oracle = playback(&project, track, &oracle_input);
        let mut expected = [[0.0; 2]; 64];
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            output, expected,
            "fresh epoch must begin at its first sample"
        );

        clock.revoke_reserved_prefix();
        input.reactivate_on_control().unwrap();
        input.set_key(0, Some((64, 100, 3))).unwrap();
        clock.revoke_reserved_prefix();
        actual.apply_priority_stop(&clock);
        actual.render(&mut output).unwrap();
        assert_eq!(
            output, [[0.0; 2]; 64],
            "repeated Stop revokes all preceding epochs"
        );
        input.reactivate_on_control().unwrap();
        actual.apply(PlaybackCommand::Stop);
        assert!(
            input.is_cancelled(),
            "direct engine Stop remains terminal even for a bound root"
        );
        assert!(input.reactivate_on_control().is_err());
    }

    #[test]
    fn output_keyboard_deferred_note_write_preserves_phase_but_new_epoch_cannot_reuse_old_words() {
        let (project, track) = fixture();
        let clock = Arc::new(KeyboardActivationClock::default());
        let input =
            Arc::new(ComputerKeyboardInput::with_output_activation(Arc::clone(&clock)).unwrap());
        let oracle_input = Arc::new(ComputerKeyboardInput::default());
        for input in [&input, &oracle_input] {
            input.set_key(0, Some((60, 100, 1))).unwrap();
        }
        let mut actual = playback(&project, track, &input);
        let mut oracle = playback(&project, track, &oracle_input);
        let mut output = [[0.0; 2]; 67];
        let mut expected = output;
        actual.render(&mut output).unwrap();
        oracle.render(&mut expected).unwrap();
        let after = input.begin_write().unwrap(); // deterministic paused sole control writer
        actual.render(&mut output).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            output, expected,
            "deferred note write must not restart a held voice"
        );
        input
            .sequence
            .store(after, std::sync::atomic::Ordering::SeqCst);
        clock.revoke_reserved_prefix();
        input.reactivate_on_control().unwrap();
        let after = input.begin_write().unwrap();
        actual.render(&mut output).unwrap();
        assert_eq!(
            output, [[0.0; 2]; 67],
            "incomplete new activation cannot reuse old desired words"
        );
        input
            .sequence
            .store(after, std::sync::atomic::Ordering::SeqCst);
        input.cancel();
        assert!(input.reactivate_on_control().is_err());
        actual.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 67]);
    }

    #[test]
    fn computer_keyboard_voice_pressure_rejects_held_press_until_new_press_without_sticking() {
        let (project, track) = fixture();
        let input = Arc::new(ComputerKeyboardInput::default());
        let release = ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input))
            .unwrap()
            .release_frames;
        let mut playback = playback(&project, track, &input);
        for token in 1..=80 {
            input.set_key(0, Some((60, 100, token))).unwrap();
            playback.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        assert!(input.rejected());
        let mut output = [[1.0; 2]; 256];
        // Voice pressure rejects this physical press, while already admitted
        // releases finish normally through the source instead of resetting it.
        for _ in 0..release.div_ceil(256) + 1 {
            playback.render(&mut output).unwrap();
        }
        assert_eq!(
            output, [[0.0; 2]; 256],
            "a held rejected press cannot start later"
        );
        input.set_key(0, None).unwrap();
        playback.render(&mut output).unwrap();
        input.set_key(0, Some((60, 100, 81))).unwrap();
        playback.render(&mut output).unwrap();
        assert!(
            output
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.000_001)
        );
        input.set_key(0, None).unwrap();
        for _ in 0..release.div_ceil(256) + 1 {
            playback.render(&mut output).unwrap();
        }
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
    fn computer_keyboard_mutable_nyquist_note_is_silenced_and_audio_stays_usable() {
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
        // Every operator is above 4 kHz nyquist: silenced, not refused (Pulse #3957).
        assert!(!input.rejected());
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
