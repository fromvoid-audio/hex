use super::*;

use crate::routed_midi_fixture;

mod authored_history;

mod cue;

mod frozen_instrument_parameters;

mod instrument_matrix_ramp;

mod interactive_start;

mod release_timeline;

pub(super) mod session_note_chase;

mod tempo_continuity;

use crate::MidiTiming;

use crate::compile::PreparedAudioSource;

use crate::playback::{Playback, PlaybackAction, PlaybackCommand};

use crate::{BorrowedMediaPage, MediaPages, StreamSourceKey, StreamedSourceMetadata};

use crate::{PreparedRevision, PreparedSessionInventory};

use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipSnapshot, Command, Created, DeviceSnapshot, Editor,
    LoopRegion, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, MidiSourceSnapshot, Project,
    SceneSnapshot, SessionCellContentSnapshot, SessionCellSnapshot, SessionMidiClipSnapshot,
    SessionSnapshot, Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

thread_local! {
    static MUTABLE_RUNTIME_CREATE_COUNTER: RefCell<Option<Arc<AtomicUsize>>> = const {
        RefCell::new(None)
    };
    static MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE: Cell<Option<usize>> = const {
        Cell::new(None)
    };
    static MUTABLE_EVENT_CAPACITY_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
    static MUTABLE_VOICE_CAPACITY_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
}

#[derive(Default)]
struct ContinuityMissingPages;

impl MediaPages for ContinuityMissingPages {
    fn page(&mut self, _key: StreamSourceKey, _page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        None
    }

    fn request_page(&mut self, _key: StreamSourceKey, _page_index: u64) {}
}

struct ContinuityInstrumentDefinition {
    id: hex_lattice_contract::DeviceTypeId,
    levels: [f32; 2],
    release_frames: u64,
}

struct ContinuityInstrument {
    level: f32,
    release_frames: u64,
}

impl hex_lattice_contract::NativeDeviceMetadata for ContinuityInstrumentDefinition {
    fn type_id(&self) -> hex_lattice_contract::DeviceTypeId {
        self.id
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("continuity test instrument")
    }

    fn state_version(&self) -> u32 {
        1
    }

    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        &[]
    }

    fn default_state(
        &self,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        hex_lattice_contract::NativeDeviceState::new(self.id, 1, vec![0])
    }

    fn migrate_state(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        if state.type_id() == self.id
            && state.state_version() == 1
            && state.payload().len() == 1
            && state.payload()[0] < 2
        {
            Ok(state.clone())
        } else {
            Err(hex_lattice_contract::DeviceContractError::MalformedState)
        }
    }

    fn read_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: hex_lattice_contract::ParameterId,
    ) -> Result<hex_lattice_contract::ParameterValue, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }

    fn patch_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: hex_lattice_contract::ParameterId,
        _: hex_lattice_contract::ParameterValue,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }
}

impl hex_lattice::NativeInstrumentDefinition for ContinuityInstrumentDefinition {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if !(8_000..=192_000).contains(&sample_rate)
            || state.type_id() != self.id
            || state.state_version() != 1
            || state.payload().len() != 1
        {
            return Err(hex_lattice_contract::DeviceContractError::MalformedState);
        }
        let level = self
            .levels
            .get(usize::from(state.payload()[0]))
            .copied()
            .ok_or(hex_lattice_contract::DeviceContractError::MalformedState)?;
        Ok(Box::new(ContinuityInstrument {
            level,
            release_frames: self.release_frames,
        }))
    }
}

impl hex_lattice::PreparedNativeInstrument for ContinuityInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(self.level.to_bits());
        Ok(hex_lattice::PreparedAnalyticNote::new(
            payload,
            self.release_frames,
            continuity_sample,
        ))
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<
        Option<Box<dyn hex_lattice::PreparedSharedAnalyticContext>>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Some(Box::new(ContinuitySharedContext {
            level: self.level,
            release_frames: self.release_frames,
        })))
    }
}

struct ContinuitySharedContext {
    level: f32,
    release_frames: u64,
}

struct OversizedSharedContextDefinition;

struct OversizedSharedContext;

impl hex_lattice_contract::NativeDeviceMetadata for OversizedSharedContextDefinition {
    fn type_id(&self) -> hex_lattice_contract::DeviceTypeId {
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.oversized-context")
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("oversized shared context test")
    }

    fn state_version(&self) -> u32 {
        1
    }

    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        &[]
    }

    fn default_state(
        &self,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        hex_lattice_contract::NativeDeviceState::new(self.type_id(), 1, vec![0])
    }

    fn migrate_state(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        if state.type_id() == self.type_id() && state.state_version() == 1 {
            Ok(state.clone())
        } else {
            Err(hex_lattice_contract::DeviceContractError::MalformedState)
        }
    }

    fn read_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: hex_lattice_contract::ParameterId,
    ) -> Result<hex_lattice_contract::ParameterValue, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }

    fn patch_parameter(
        &self,
        _: &hex_lattice_contract::NativeDeviceState,
        _: hex_lattice_contract::ParameterId,
        _: hex_lattice_contract::ParameterValue,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        Err(hex_lattice_contract::DeviceContractError::UnknownParameter)
    }
}

impl hex_lattice::NativeInstrumentDefinition for OversizedSharedContextDefinition {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        hex_lattice_contract::NativeDeviceMetadata::migrate_state(self, state)?;
        Ok(Box::new(OversizedSharedContext))
    }
}

impl hex_lattice::PreparedNativeInstrument for OversizedSharedContext {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<
        Option<Box<dyn hex_lattice::PreparedSharedAnalyticContext>>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Some(Box::new(OversizedSharedContext)))
    }
}

impl hex_lattice::PreparedSharedAnalyticContext for OversizedSharedContext {
    fn declared_context_bytes(&self) -> usize {
        hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES + 1
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedSharedAnalyticNote, hex_lattice_contract::DeviceContractError>
    {
        Ok(hex_lattice::PreparedSharedAnalyticNote::new([0; 8], 0))
    }

    fn sample(
        &self,
        _: &hex_lattice::PreparedSharedAnalyticNote,
        _: u64,
        _: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> [f32; 2] {
        [0.0; 2]
    }
}

impl hex_lattice::PreparedSharedAnalyticContext for ContinuitySharedContext {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedSharedAnalyticNote, hex_lattice_contract::DeviceContractError>
    {
        let mut payload = [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(self.level.to_bits());
        Ok(hex_lattice::PreparedSharedAnalyticNote::new(
            payload,
            self.release_frames,
        ))
    }

    fn sample(
        &self,
        note: &hex_lattice::PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> [f32; 2] {
        if age < gate.saturating_add(note.release_frames()) {
            let level = f32::from_bits(note.payload()[0] as u32);
            [level; 2]
        } else {
            [0.0; 2]
        }
    }
}

fn continuity_sample(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    release: u64,
    _resources: &dyn hex_lattice::DeviceResources,
) -> [f32; 2] {
    if age < gate.saturating_add(release) {
        let level = f32::from_bits(payload[0] as u32);
        [level; 2]
    } else {
        [0.0; 2]
    }
}

struct MutableFeedbackDefinition {
    metadata: ContinuityInstrumentDefinition,
}

// Reuse the finite source law in cross-owner playback dispatch tests.
pub(super) fn finite_feedback_definition(
    release_frames: u64,
) -> impl hex_lattice::NativeInstrumentDefinition {
    MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback"),
            levels: [0.2, 0.4],
            release_frames,
        },
    }
}

struct MutableFeedbackPrepared {
    level: f32,
    release_frames: u64,
}

struct MutableFeedbackFactory {
    level: f32,
    release_frames: u64,
}

#[derive(Clone, Copy, Default)]
struct MutableFeedbackVoice {
    occurrence_id: u64,
    pitch: u8,
    velocity: u8,
    active: bool,
    gated: bool,
    release_left: u8,
}

struct MutableFeedbackRuntime {
    key: u32,
    level: f32,
    release_frames: u8,
    voices: [MutableFeedbackVoice; 64],
    feedback: f32,
    tail_left: u8,
    // Optional real owned storage exercises variable-sized runtime accounting.
    storage: Box<[u8]>,
}

/// Minimal mutable voice used to exercise an instrument with a real declared
/// zero-frame release. Unlike `MutableFeedbackRuntime`, it has no feedback or
/// post-gate tail, so the fixture's declared bound is truthful.
#[derive(Clone, Copy, Default)]
struct ZeroTailMutableRuntime {
    active: bool,
}

struct ZeroTailMutableFactory;

impl hex_lattice::PreparedMutableInstrumentRuntime for ZeroTailMutableFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<ZeroTailMutableRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.get().unwrap_or(256))
    }
    fn max_active_occurrences(&self) -> usize {
        MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.get().unwrap_or(64))
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        MUTABLE_RUNTIME_CREATE_COUNTER.with(|counter| {
            if let Some(counter) = counter.borrow().as_ref() {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        Box::new(ZeroTailMutableRuntime::default())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl hex_lattice::MutableInstrumentRuntime for ZeroTailMutableRuntime {
    fn preflight_events(
        &self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        let mut active = self.active;
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let (offset, is_on) = match event {
                hex_lattice::NativeInstrumentEvent::NotePressure { .. }
                | hex_lattice::NativeInstrumentEvent::Expression { .. }
                | hex_lattice::NativeInstrumentEvent::ControlChange { .. }
                | hex_lattice::NativeInstrumentEvent::PitchBend { .. }
                | hex_lattice::NativeInstrumentEvent::NotePitchBend { .. }
                | hex_lattice::NativeInstrumentEvent::NoteTimbre { .. } => {
                    return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. } => {
                    (frame_offset, true)
                }
                hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. } => {
                    (frame_offset, false)
                }
            };
            if offset < previous_offset || offset >= frame_count {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            if is_on == active {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            active = is_on;
            previous_offset = offset;
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while let Some(event) = events.get(event_index).copied() {
                let (offset, is_on) = match event {
                    hex_lattice::NativeInstrumentEvent::NotePressure { .. }
                    | hex_lattice::NativeInstrumentEvent::Expression { .. }
                    | hex_lattice::NativeInstrumentEvent::ControlChange { .. }
                    | hex_lattice::NativeInstrumentEvent::PitchBend { .. }
                    | hex_lattice::NativeInstrumentEvent::NotePitchBend { .. }
                    | hex_lattice::NativeInstrumentEvent::NoteTimbre { .. } => return,
                    hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. } => {
                        (frame_offset, true)
                    }
                    hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. } => {
                        (frame_offset, false)
                    }
                };
                if offset != frame_index {
                    break;
                }
                self.active = is_on;
                event_index += 1;
            }
            if self.active {
                frame[0] += 0.25;
                frame[1] += 0.25;
            }
        }
    }

    fn reset(&mut self) {
        self.active = false;
    }
    fn is_quiescent(&self) -> bool {
        !self.active
    }
    fn can_copy_history_from(&self, previous: &dyn hex_lattice::MutableInstrumentRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn hex_lattice::MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.active = previous.active;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl MutableFeedbackRuntime {
    fn advance(voices: &mut [MutableFeedbackVoice; 64], frames: usize) {
        for _ in 0..frames {
            for voice in voices.iter_mut() {
                if voice.active && !voice.gated {
                    voice.release_left = voice.release_left.saturating_sub(1);
                    if voice.release_left == 0 {
                        *voice = MutableFeedbackVoice::default();
                    }
                }
            }
        }
    }

    fn apply(
        voices: &mut [MutableFeedbackVoice; 64],
        event: hex_lattice::NativeInstrumentEvent,
        release: u8,
    ) {
        match event {
            hex_lattice::NativeInstrumentEvent::NotePressure { .. }
            | hex_lattice::NativeInstrumentEvent::Expression { .. }
            | hex_lattice::NativeInstrumentEvent::ControlChange { .. }
            | hex_lattice::NativeInstrumentEvent::PitchBend { .. }
            | hex_lattice::NativeInstrumentEvent::NotePitchBend { .. }
            | hex_lattice::NativeInstrumentEvent::NoteTimbre { .. } => {}
            hex_lattice::NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if let Some(voice) = voices.iter_mut().find(|voice| !voice.active) {
                    *voice = MutableFeedbackVoice {
                        occurrence_id,
                        pitch,
                        velocity,
                        active: true,
                        gated: true,
                        release_left: release,
                    };
                }
            }
            hex_lattice::NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = voices
                    .iter_mut()
                    .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                {
                    voice.gated = false;
                    voice.release_left = release;
                }
            }
        }
    }
}

impl hex_lattice_contract::NativeDeviceMetadata for MutableFeedbackDefinition {
    fn type_id(&self) -> hex_lattice_contract::DeviceTypeId {
        self.metadata.id
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("mutable feedback proof")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [hex_lattice_contract::ParameterDescriptor] {
        &[]
    }
    fn default_state(
        &self,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.metadata.default_state()
    }
    fn migrate_state(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.metadata.migrate_state(state)
    }
    fn read_parameter(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
    ) -> Result<hex_lattice_contract::ParameterValue, hex_lattice_contract::DeviceContractError>
    {
        self.metadata.read_parameter(state, id)
    }
    fn patch_parameter(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        id: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    ) -> Result<hex_lattice_contract::NativeDeviceState, hex_lattice_contract::DeviceContractError>
    {
        self.metadata.patch_parameter(state, id, value)
    }
}

impl hex_lattice::NativeInstrumentDefinition for MutableFeedbackDefinition {
    fn prepare_instrument(
        &self,
        state: &hex_lattice_contract::NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if !(8_000..=192_000).contains(&sample_rate)
            || state.type_id() != self.metadata.id
            || state.state_version() != 1
        {
            return Err(hex_lattice_contract::DeviceContractError::MalformedState);
        }
        let level = *self
            .metadata
            .levels
            .get(usize::from(*state.payload().first().ok_or(
                hex_lattice_contract::DeviceContractError::MalformedState,
            )?))
            .ok_or(hex_lattice_contract::DeviceContractError::MalformedState)?;
        Ok(Box::new(MutableFeedbackPrepared {
            level,
            release_frames: self.metadata.release_frames,
        }))
    }
}

impl hex_lattice::PreparedNativeInstrument for MutableFeedbackPrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<
        Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>,
        hex_lattice_contract::DeviceContractError,
    > {
        if self.release_frames == 0 {
            Ok(Some(Box::new(ZeroTailMutableFactory)))
        } else {
            Ok(Some(Box::new(MutableFeedbackFactory {
                level: self.level,
                release_frames: self.release_frames,
            })))
        }
    }
}

impl hex_lattice::PreparedMutableInstrumentRuntime for MutableFeedbackFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| {
            bytes
                .get()
                .unwrap_or(std::mem::size_of::<MutableFeedbackRuntime>())
        })
    }
    fn max_events_per_block(&self) -> usize {
        MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.get().unwrap_or(256))
    }
    fn max_active_occurrences(&self) -> usize {
        MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.get().unwrap_or(64))
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release_frames + 8
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        MUTABLE_RUNTIME_CREATE_COUNTER.with(|counter| {
            if let Some(counter) = counter.borrow().as_ref() {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        Box::new(MutableFeedbackRuntime {
            key: self.level.to_bits(),
            level: self.level,
            release_frames: u8::try_from(self.release_frames).unwrap_or(u8::MAX),
            voices: [MutableFeedbackVoice::default(); 64],
            feedback: 0.0,
            tail_left: 0,
            storage: vec![
                0;
                self.declared_twin_bytes()
                    .saturating_sub(std::mem::size_of::<MutableFeedbackRuntime>())
            ]
            .into_boxed_slice(),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl hex_lattice::MutableInstrumentRuntime for MutableFeedbackRuntime {
    fn preflight_events(
        &self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        let mut voices = self.voices;
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let offset = event.frame_offset();
            if offset < previous_offset || offset >= frame_count {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            Self::advance(&mut voices, offset - previous_offset);
            match event {
                hex_lattice::NativeInstrumentEvent::NotePressure { .. }
                | hex_lattice::NativeInstrumentEvent::Expression { .. }
                | hex_lattice::NativeInstrumentEvent::ControlChange { .. }
                | hex_lattice::NativeInstrumentEvent::PitchBend { .. }
                | hex_lattice::NativeInstrumentEvent::NotePitchBend { .. }
                | hex_lattice::NativeInstrumentEvent::NoteTimbre { .. } => {
                    return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                hex_lattice::NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if voices
                        .iter()
                        .any(|voice| voice.active && voice.occurrence_id == occurrence_id)
                    {
                        return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    if !voices.iter().any(|voice| !voice.active) {
                        return Err(
                            hex_lattice::NativeInstrumentRuntimeError::EventCapacityExceeded,
                        );
                    }
                }
                hex_lattice::NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if !voices.iter().any(|voice| {
                        voice.active && voice.gated && voice.occurrence_id == occurrence_id
                    }) {
                        return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
            }
            Self::apply(&mut voices, event, self.release_frames);
            previous_offset = offset;
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while events
                .get(event_index)
                .is_some_and(|event| event.frame_offset() == frame_index)
            {
                Self::apply(&mut self.voices, events[event_index], self.release_frames);
                event_index += 1;
            }
            let active = self.voices.iter().any(|voice| voice.active);
            let direct = self
                .voices
                .iter()
                .filter(|voice| voice.active)
                .map(|voice| {
                    let pitch_scale = 1.0 + (f32::from(voice.pitch) - 60.0) * 0.01;
                    self.level * (f32::from(voice.velocity) / 100.0) * pitch_scale
                })
                .sum::<f32>();
            self.feedback = self.feedback * 0.5 + direct * 0.25;
            let contribution = direct + self.feedback;
            frame[0] += contribution;
            frame[1] += contribution;
            Self::advance(&mut self.voices, 1);
            if active && self.voices.iter().all(|voice| !voice.active) {
                self.tail_left = 8;
            }
            if self.voices.iter().all(|voice| !voice.active) && self.tail_left > 0 {
                self.tail_left -= 1;
                if self.tail_left == 0 {
                    self.feedback = 0.0;
                }
            }
        }
    }
    fn reset(&mut self) {
        self.voices = [MutableFeedbackVoice::default(); 64];
        self.feedback = 0.0;
        self.tail_left = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| !voice.active) && self.tail_left == 0 && self.feedback == 0.0
    }
    fn can_copy_history_from(&self, previous: &dyn hex_lattice::MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.key == old.key
                && self.release_frames == old.release_frames
                && self.storage.len() == old.storage.len()
        })
    }
    fn copy_history_from(&mut self, previous: &dyn hex_lattice::MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
            self.feedback = old.feedback;
            self.tail_left = old.tail_left;
            self.storage.copy_from_slice(&old.storage);
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.storage.len()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// These hand-authored fixtures model the schema 1–14
/// migration that installs a default sine slot on every track carrying MIDI.
fn legacy_project_from_snapshot(mut snapshot: Snapshot) -> Result<Project, hex_project::Invalid> {
    let state = hex_project::default_instrument_state()?;
    for track in &mut snapshot.tracks {
        let has_midi = match &track.content {
            TrackContentSnapshot::Instrument(clips) => !clips.is_empty(),
            TrackContentSnapshot::Hybrid { midi_clips, .. } => !midi_clips.is_empty(),
            TrackContentSnapshot::Audio(_) => false,
        } || snapshot.session.cells.iter().any(|cell| {
            cell.track == track.id && matches!(cell.content, SessionCellContentSnapshot::Midi(_))
        });
        if has_midi && track.instrument.is_none() {
            let id = snapshot.next_id;
            snapshot.next_id = snapshot
                .next_id
                .checked_add(1)
                .ok_or(hex_project::Invalid("fixture id capacity"))?;
            track.instrument = Some(DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id,
                bypassed: false,
                state: state.clone(),
            });
            track.instrument_position = Some(0);
        }
    }
    Project::from_legacy_snapshot(snapshot)
}

fn project(gain: f32, note_start: u64, note_duration: u64) -> Project {
    project_at_tempo(gain, note_start, note_duration, Tempo::DEFAULT)
}

fn audio_project(source_start: u64, source_end: u64, asset_id: u64) -> Project {
    legacy_project_from_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: 7,
        assets: vec![
            AssetSnapshot {
                original_source: None,
                id: 4,
                path: "shared.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
            AssetSnapshot {
                original_source: None,
                id: 5,
                path: "replacement.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
        ],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 1,
            name: "audio".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(Vec::new()),
            sends: Vec::new(),
        }],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 2,
                name: "scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: 1,
                scene: 2,
                content: SessionCellContentSnapshot::Audio(hex_project::SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: 3,
                    name: "one shot".into(),
                    asset: asset_id,
                    source_start,
                    source_end,
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })
    .expect("valid Session audio project")
}

fn audio_media(
    project: &Project,
    asset4: &crate::Audio,
    asset5: &crate::Audio,
) -> BTreeMap<hex_project::AssetId, crate::Audio> {
    project
        .assets()
        .map(|asset| {
            (
                asset.id(),
                if asset.id().value() == 4 {
                    asset4.clone()
                } else {
                    asset5.clone()
                },
            )
        })
        .collect()
}

fn mixed_audio_midi_project(source_start: u64, source_end: u64) -> Project {
    let mut snapshot = audio_project(source_start, source_end, 4).snapshot();
    snapshot.next_id = 12;
    snapshot.tracks.push(TrackSnapshot {
        group_output_mutes: Vec::new(),
        tags: Vec::new(),
        kind: hex_project::TrackKind::Ordinary,
        parent_group: None,
        color: hex_project::ElementColor::default(),
        gain_envelope: Vec::new(),
        gain_touch_spans: Vec::new(),
        gain_touch_epoch: 0,
        id: 6,
        name: "keys".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
            content_view: None,
            active: true,
            playback_offset_raw: 0,
            one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
            volume: hex_project::MidiClipVolume::default(),
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            id: 8,
            start_raw: 0,
            length_raw: 4 << 32,
            source_length_raw: 4 << 32,
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: 4 << 32,
            looping: false,
            notes: vec![MidiNoteSnapshot {
                active: true,
                id: 9,
                start_raw: 0,
                duration_raw: 1 << 32,
                pitch: 69,
                channel: 0,
                velocity: 100,
                expression: Default::default(),
                expression_points: Vec::new(),
            }],
            expression_lanes: Vec::new(),
        }]),
        sends: Vec::new(),
    });
    snapshot.session.scenes[0].name = "audio and keys".into();
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 6,
        scene: 2,
        content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
            active: true,
            playback_offset_raw: 0,
            volume: hex_project::MidiClipVolume::default(),
            flavor: hex_project::ClipFlavor::default(),
            id: 10,
            name: "empty midi".into(),
            source: MidiSourceSnapshot {
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                source_length_raw: 4 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 4 << 32,
                looping: false,
                notes: Vec::new(),
                expression_lanes: Vec::new(),
            },
        }),
    });
    snapshot.session.scenes.push(SceneSnapshot {
        id: 7,
        name: "stop audio".into(),
    });
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 1,
        scene: 7,
        content: SessionCellContentSnapshot::Stop,
    });
    legacy_project_from_snapshot(snapshot).expect("mixed audio and midi project")
}

fn session_audio_midi_arrangement_mix_project() -> Project {
    let mut snapshot = mixed_audio_midi_project(5, 18).snapshot();
    snapshot.next_id = 16;
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: 11,
        start_raw: 0,
        duration_raw: 1 << 32,
        pitch: 69,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    snapshot.tracks.push(TrackSnapshot {
        group_output_mutes: Vec::new(),
        tags: Vec::new(),
        kind: hex_project::TrackKind::Ordinary,
        parent_group: None,
        color: hex_project::ElementColor::default(),
        gain_envelope: Vec::new(),
        gain_touch_spans: Vec::new(),
        gain_touch_epoch: 0,
        id: 14,
        name: "arrangement bed".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: 15,
            asset: 4,
            start: 0,
            duration: 32,
            source_offset: 0,
            source_length: 32,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(32).unwrap(),
        }]),
        sends: Vec::new(),
    });
    legacy_project_from_snapshot(snapshot)
        .expect("mixed Session and Arrangement audio/MIDI project")
}

fn project_at_tempo(gain: f32, note_start: u64, note_duration: u64, tempo: Tempo) -> Project {
    legacy_project_from_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: 8,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 1,
            name: "track".into(),
            gain,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: 2,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    active: true,
                    id: 3,
                    start_raw: note_start,
                    duration_raw: note_duration,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                    expression: Default::default(),
                    expression_points: Vec::new(),
                }],
                expression_lanes: Vec::new(),
            }]),
            sends: Vec::new(),
        }],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 4,
                name: "scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: 1,
                scene: 4,
                content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                    active: true,
                    playback_offset_raw: 0,
                    volume: hex_project::MidiClipVolume::default(),
                    flavor: hex_project::ClipFlavor::default(),
                    id: 5,
                    name: "clip".into(),
                    source: MidiSourceSnapshot {
                        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                        source_length_raw: 32 << 32,
                        source_start_raw: 0,
                        loop_start_raw: 0,
                        loop_end_raw: 32 << 32,
                        looping: false,
                        notes: vec![MidiNoteSnapshot {
                            active: true,
                            id: 6,
                            start_raw: note_start,
                            duration_raw: note_duration,
                            pitch: 60,
                            channel: 0,
                            velocity: 100,
                            expression: Default::default(),
                            expression_points: Vec::new(),
                        }],
                        expression_lanes: Vec::new(),
                    },
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, tempo)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })
    .unwrap()
}

fn with_session_note_id(project: &Project, note_id: u64) -> Project {
    let mut snapshot = project.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes[0].id = note_id;
    }
    snapshot.next_id = note_id + 1;
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn with_instrument_bypassed(project: &Project, bypassed: bool) -> Project {
    let mut snapshot = project.snapshot();
    let instrument = snapshot.tracks[0]
        .instrument
        .as_mut()
        .expect("MIDI fixture has a canonical instrument slot");
    instrument.bypassed = bypassed;
    Project::from_legacy_snapshot(snapshot).unwrap()
}

fn session_only_project(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn without_instrument_slot(project: &Project) -> Project {
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(hex_project::Command::RemoveInstrument { track })
        .unwrap();
    editor.project().clone()
}

fn with_inserted_instrument_slot(
    project: &Project,
    state: hex_lattice_contract::NativeDeviceState,
) -> Project {
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(hex_project::Command::InsertInstrument { track, state })
        .unwrap();
    editor.project().clone()
}

fn with_replaced_instrument_id(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    let replacement_id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0]
        .instrument
        .as_mut()
        .expect("MIDI fixture has a canonical instrument slot")
        .id = replacement_id;
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn with_second_session_note(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    let note_id = snapshot.next_id;
    snapshot.next_id += 1;
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture has a Session MIDI clip")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: note_id,
        start_raw: 1 << 32,
        duration_raw: 1 << 32,
        pitch: 64,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn with_second_arrangement_note(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    let note_id = snapshot.next_id;
    snapshot.next_id = snapshot.next_id.checked_add(1).unwrap();
    let clips = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips,
        TrackContentSnapshot::Audio(_) => panic!("fixture has Arrangement MIDI content"),
    };
    clips[0].notes.push(MidiNoteSnapshot {
        active: true,
        id: note_id,
        start_raw: 1 << 32,
        duration_raw: 1 << 32,
        pitch: 64,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    Project::from_legacy_snapshot(snapshot).unwrap()
}

fn with_session_note_at(project: &Project, start_raw: u64) -> Project {
    let mut snapshot = project.snapshot();
    let note_id = snapshot.next_id;
    snapshot.next_id += 1;
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture has a Session MIDI clip")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: note_id,
        start_raw,
        duration_raw: 1 << 32,
        pitch: 64,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn with_test_instrument(
    project: &Project,
    type_id: hex_lattice_contract::DeviceTypeId,
    state_value: u8,
) -> Project {
    let mut snapshot = project.snapshot();
    let instrument = snapshot.tracks[0]
        .instrument
        .as_mut()
        .expect("MIDI fixture has an instrument slot");
    instrument.state =
        hex_lattice_contract::NativeDeviceState::new(type_id, 1, vec![state_value]).unwrap();
    Project::from_legacy_snapshot(snapshot).unwrap()
}

fn revision(project: &Project, table: Arc<hex_devices::SineTable>) -> Arc<PreparedRevision> {
    Arc::new(PreparedRevision::prepare(project, &BTreeMap::new(), table, None).unwrap())
}

fn revision_with_instrument_registry(
    project: &Project,
    table: Arc<hex_devices::SineTable>,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            project,
            &BTreeMap::new(),
            table,
            None,
            registry,
        )
        .unwrap(),
    )
}

// Exercise the actual ordinary compiler policy with an empty resource set. No descriptor is
// fabricated and the closed/headless preparation path remains independently conservative.
fn ordinary_mutable_playback(
    project: &Project,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> SessionPlayback {
    let cancel = AtomicBool::new(false);
    let resources = crate::native_device_resources::PreparedDeviceResources::prepare_on_control(
        &crate::native_device_resources::ResidentDeviceAssets::default(),
        project,
        1 << 20,
        &cancel,
    )
    .unwrap();
    let preparation =
        crate::native_device_resources::CompilerDeviceResources::new(&resources, 0, &cancel);
    let effects = hex_devices::native_registry().unwrap();
    let revision = PreparedRevision::prepare_with_registries_and_sampler_sources(
        project,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &effects,
        registry,
        &crate::audio_transform::AudioTransformPreparation::default(),
        Some(&preparation),
    )
    .unwrap();
    assert!(revision.device_handoff_metadata_bytes().unwrap() > 0);
    SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
}

fn mutable_hundred_track_preroll_revision() -> Arc<PreparedRevision> {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0).snapshot();
    let mut next_id = snapshot.next_id;
    fn remap_midi_clips(clips: &mut [hex_project::MidiClipSnapshot], next_id: &mut u64) {
        for clip in clips {
            clip.id = *next_id;
            *next_id += 1;
            for note in &mut clip.notes {
                note.id = *next_id;
                *next_id += 1;
            }
        }
    }
    for track_index in 1..100 {
        let mut track = snapshot.tracks[0].clone();
        track.id = next_id;
        next_id += 1;
        track.name = format!("track {track_index}");
        for device in &mut track.devices {
            device.id = next_id;
            next_id += 1;
        }
        if let Some(instrument) = track.instrument.as_mut() {
            instrument.id = next_id;
            next_id += 1;
        }
        match &mut track.content {
            TrackContentSnapshot::Audio(clips) => {
                for clip in clips {
                    clip.id = next_id;
                    next_id += 1;
                }
            }
            TrackContentSnapshot::Instrument(clips) => remap_midi_clips(clips, &mut next_id),
            TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } => {
                for clip in audio_clips {
                    clip.id = next_id;
                    next_id += 1;
                }
                remap_midi_clips(midi_clips, &mut next_id);
            }
        }
        snapshot.tracks.push(track);
    }
    snapshot.next_id = next_id;
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
    revision_with_instrument_registry(&project, table, &registry)
}

fn mutable_checkpoint_fixture(note_duration: u64) -> (Arc<PreparedRevision>, hex_project::TrackId) {
    mutable_checkpoint_fixture_with_release(note_duration, 8)
}

fn mutable_checkpoint_fixture_with_release(
    note_duration: u64,
    release_frames: u64,
) -> (Arc<PreparedRevision>, hex_project::TrackId) {
    mutable_checkpoint_fixture_at(0, note_duration, release_frames)
}

fn mutable_checkpoint_fixture_at(
    note_start: u64,
    note_duration: u64,
    release_frames: u64,
) -> (Arc<PreparedRevision>, hex_project::TrackId) {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let project = with_test_instrument(&project(1.0, note_start, note_duration), id, 0);
    let revision = revision_with_instrument_registry(&project, table, &registry);
    let track = revision.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    (revision, track)
}

fn render_checkpoint_prefix(playback: &mut SessionPlayback, frames: usize) -> Vec<crate::Frame> {
    let mut rendered = vec![[0.0; 2]; frames];
    playback.render(&mut rendered).unwrap();
    rendered
}

pub(super) fn render_checkpoint_partitioned(
    playback: &mut SessionPlayback,
    pattern: &[usize],
    total_frames: usize,
) -> Vec<crate::Frame> {
    let mut rendered = Vec::with_capacity(total_frames);
    let mut pattern_index = 0;
    while rendered.len() < total_frames {
        let count = pattern[pattern_index % pattern.len()].min(total_frames - rendered.len());
        let mut block = vec![[0.0; 2]; count];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
        pattern_index += 1;
    }
    rendered
}

fn reset_checkpoint_track(playback: &mut SessionPlayback, track: hex_project::TrackId) {
    let route = playback
        .routes
        .iter()
        .position(|route| route.id == track)
        .unwrap();
    let factory = playback
        .revision
        .session()
        .mutable_instrument_factory(track)
        .unwrap();
    let source_live = playback.mutable_instruments[route]
        .as_ref()
        .is_some_and(MutableInstrumentTrackRuntime::is_source_owner);
    let mut fresh = crate::mutable_instrument::MutableInstrumentTrackRuntime::new(
        factory,
        playback.revision.arrangement().sample_rate(),
    )
    .unwrap();
    if source_live {
        fresh.enable_source_live();
    }
    playback.mutable_instruments[route] = Some(fresh);
}

fn with_utility_filter_state(project: &Project, dc_remove: bool, bass_mono: bool) -> Project {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut snapshot = project.snapshot();
    let mut state = UTILITY_DEFINITION.default_state().unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::DC_REMOVE,
            ParameterValue::Bool(dc_remove),
        )
        .unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::BASS_MONO,
            ParameterValue::Bool(bass_mono),
        )
        .unwrap();
    if let Some(device) = snapshot.tracks[0].devices.first_mut() {
        device.state = state;
        device.bypassed = false;
    } else {
        snapshot.tracks[0]
            .devices
            .push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state,
            });
        snapshot.next_id += 1;
    }
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn with_utility_filter_state_at(
    project: &Project,
    device_index: usize,
    dc_remove: bool,
    bass_mono: bool,
) -> Project {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut state = UTILITY_DEFINITION.default_state().unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::DC_REMOVE,
            ParameterValue::Bool(dc_remove),
        )
        .unwrap();
    state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            parameter::BASS_MONO,
            ParameterValue::Bool(bass_mono),
        )
        .unwrap();
    let mut snapshot = project.snapshot();
    let device = snapshot.tracks[0]
        .devices
        .get_mut(device_index)
        .expect("fixture device index is valid");
    device.state = state;
    device.bypassed = false;
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn without_session_cells(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    snapshot.session.cells.clear();
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn assert_quantized_scene_pcm(sample_rate: u32, cancelled: bool) {
    let due = usize::try_from(sample_rate).unwrap() * 2;
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 0.25, -(index as f32 + 0.5)])
        .collect();
    let audio = crate::Audio::new(sample_rate, source.clone()).unwrap();
    let other = crate::Audio::new(sample_rate, vec![[0.0; 2]; 32]).unwrap();
    let mut snapshot = audio_project(5, 18, 4).snapshot();
    snapshot.sample_rate = sample_rate;
    for asset in &mut snapshot.assets {
        asset.source_sample_rate = sample_rate;
    }
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    let scene = project.scenes()[0].id();
    let media = audio_media(&project, &audio, &other);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = revision.session().audio_clips()[0].id;
    let track = revision.session().audio_clips()[0].track;
    // An empty block at the exact due frame must not commit a launch;
    // cancellation before the next nonempty block must still win.
    let mut boundary = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    boundary.apply(SessionPlaybackCommand::Play).unwrap();
    boundary.render(&mut [[0.0; 2]; 1]).unwrap();
    boundary
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let mut prefix = vec![[1.0; 2]; due - 1];
    boundary.render(&mut prefix).unwrap();
    assert!(prefix.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(boundary.performance_frame(), due as u64);
    boundary.render(&mut []).unwrap();
    assert_eq!(
        boundary.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(boundary.route_status(0).unwrap().pending.is_some());
    if cancelled {
        boundary
            .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
            .unwrap();
    }
    let mut first_due = [[0.0; 2]; 1];
    boundary.render(&mut first_due).unwrap();
    assert_eq!(first_due[0], if cancelled { [0.0; 2] } else { source[5] });
    assert_eq!(
        boundary.owner(track),
        Some(if cancelled {
            SessionPlaybackOwner::Arrangement
        } else {
            SessionPlaybackOwner::Session { clip }
        })
    );
    assert!(boundary.route_status(0).unwrap().pending.is_none());
    // The full early-queued prefix is checked above. Reuse only the final
    // 1024 frames for each partition so all sizes still cross the due frame.
    for block in [1_usize, 17, 64, 128, 256, 512] {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let start = due - 1024;
        playback
            .apply(SessionPlaybackCommand::Seek(start as u64))
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 1]).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::FourQuarter,
            })
            .unwrap();
        assert_eq!(
            playback
                .route_status(0)
                .unwrap()
                .pending
                .unwrap()
                .performance_frame,
            due as u64
        );
        if cancelled {
            playback
                .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
                .unwrap();
            assert!(playback.route_status(0).unwrap().pending.is_none());
        }
        playback.render(&mut []).unwrap();
        assert_eq!(playback.performance_frame(), (start + 1) as u64);
        assert_eq!(
            playback.owner(track),
            Some(SessionPlaybackOwner::Arrangement)
        );
        let mut output = vec![[1.0; 2]; due + 20 - start - 1];
        for chunk in output.chunks_mut(block) {
            playback.render(chunk).unwrap();
        }
        for (offset, actual) in output.iter().enumerate() {
            let frame = start + offset + 1;
            let expected = if !cancelled && (due..due + 13).contains(&frame) {
                source[5 + frame - due]
            } else {
                [0.0; 2]
            };
            assert_eq!(
                *actual, expected,
                "rate={sample_rate}, cancelled={cancelled}, block={block}, frame={frame}"
            );
        }
        let expected_owner = if cancelled {
            SessionPlaybackOwner::Arrangement
        } else {
            SessionPlaybackOwner::Session { clip }
        };
        assert_eq!(playback.owner(track), Some(expected_owner));
        assert!(playback.route_status(0).unwrap().pending.is_none());
        assert_eq!(playback.performance_frame(), (due + 20) as u64);
    }
}

#[inline(never)]
fn interactive_preroll_session_command_is_available() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let source_project = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let prepared =
        revision_with_instrument_registry(&source_project, Arc::clone(&table), &registry);
    let track = prepared.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    assert!(prepared.arrangement_seek_requires_preroll());

    let destination = 192_512;
    let runtime = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
        Arc::clone(&prepared),
        destination,
        destination,
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!runtime.is_linear_arrangement_export());
    assert_eq!(
        runtime.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(runtime.performance_frame(), destination);
    assert_eq!(runtime.arrangement_frame(), destination);
    assert!(!runtime.playing());

    let mut playback = Playback::from_interactive_arrangement_preroll(runtime).unwrap();
    assert_eq!(playback.position(), destination);
    assert_eq!(playback.performance_frame(), destination);
    assert!(!playback.playing());
    let session_clip = prepared.session().midi_clips()[0].id;
    playback
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip: session_clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
}

#[inline(never)]
fn interactive_preroll_warmed_state_inherits_into_compiled_revision() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let source_project = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let prepared =
        revision_with_instrument_registry(&source_project, Arc::clone(&table), &registry);
    let track = prepared.arrangement().tracks()[0]
        .canonical_track_id()
        .unwrap();
    let mut warmed = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
        Arc::clone(&prepared),
        256,
        256,
        true,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut editor = Editor::new(source_project, 8).unwrap();
    let (_, edited_revision) = editor
        .execute_prepared(
            Command::RenameTrack {
                track,
                name: "renamed track".to_owned(),
            },
            |candidate| {
                Ok::<_, hex_project::Invalid>(revision_with_instrument_registry(
                    candidate,
                    Arc::clone(&table),
                    &registry,
                ))
            },
        )
        .unwrap();

    let mut expected_suffix = [[0.0; 2]; 193];
    {
        let mut expected = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
            Arc::clone(&edited_revision),
            256,
            256,
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
        expected.render(&mut expected_suffix).unwrap();
    }

    let mut successor = SessionPlayback::new(edited_revision).unwrap();
    successor.try_inherit_state_from(&mut warmed).unwrap();
    assert_eq!(successor.performance_frame(), 256);
    assert_eq!(successor.arrangement_frame(), 256);
    assert!(successor.playing());
    let mut actual_suffix = [[0.0; 2]; 193];
    successor.render(&mut actual_suffix).unwrap();
    assert_eq!(actual_suffix, expected_suffix);
}

fn mutable_feedback_with_stateful_track_and_return() -> Arc<PreparedRevision> {
    mutable_feedback_with_stateful_track_and_return_edited(|_| {})
}

pub(super) fn mutable_feedback_with_stateful_track_and_return_edited(
    edit: impl FnOnce(&mut Snapshot),
) -> Arc<PreparedRevision> {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0).snapshot();
    let track_effect_id = snapshot.next_id;
    let return_id = track_effect_id + 1;
    let return_effect_id = track_effect_id + 2;
    snapshot.next_id = track_effect_id + 3;
    let filter_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: track_effect_id,
        bypassed: false,
        state: filter_state.clone(),
    });
    snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: return_id,
            gain: 0.5,
        });
    snapshot
        .return_tracks
        .push(hex_project::ReturnTrackSnapshot {
            id: return_id,
            name: "return".into(),
            gain: 0.75,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: return_effect_id,
                bypassed: false,
                state: filter_state,
            }],
        });
    edit(&mut snapshot);
    let source = Project::from_legacy_snapshot(snapshot).unwrap();
    revision_with_instrument_registry(&source, table, &registry)
}

fn run_preroll_continuation_on_worker_stack(prepared: Arc<PreparedRevision>, target: u64) {
    // Mirrors the dedicated control-worker reservation in hex-session; this is
    // per-test, never a global harness stack override.
    const SEEK_STACK_BYTES: usize = 8 * 1024 * 1024;
    thread::Builder::new()
        .name("hex-engine-interactive-preroll-test".into())
        .stack_size(SEEK_STACK_BYTES)
        .spawn(move || {
            assert_preroll_continuation(&prepared, target, false);
            assert_preroll_continuation(&prepared, target, true);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[inline(never)]
fn assert_loop_preroll_continuation(
    prepared: &Arc<PreparedRevision>,
    requested: u64,
    destination: u64,
    playing: bool,
) {
    // Play-from keeps absolute transport before, inside and after the loop
    // (interactive_start tests); seeks share that empty-tail law.
    let mut oracle = SessionPlayback::new(Arc::clone(prepared)).unwrap();
    oracle.play_from(destination).unwrap();
    let expected = render_checkpoint_partitioned(&mut oracle, &[17, 61, 193], 2500);
    let runtime = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
        Arc::clone(prepared),
        requested,
        requested,
        playing,
        &AtomicBool::new(false),
    )
    .expect("loop-enabled mutable and stateful seek is admitted");
    assert_eq!(runtime.arrangement_loop_pass, 0);
    assert_eq!(runtime.device_chains[0].stateful_count(), 1);
    assert_eq!(runtime.return_chains[0].stateful_count(), 1);
    let mut candidate = Playback::from_interactive_arrangement_preroll(runtime).unwrap();
    assert!(Arc::ptr_eq(
        candidate.prepared_revision().unwrap(),
        prepared
    ));
    assert_eq!(candidate.position(), destination);
    assert_eq!(candidate.performance_frame(), destination);
    assert_eq!(candidate.playing(), playing);
    if !playing {
        let mut silence = [[1.0; 2]; 19];
        candidate.render(&mut silence).unwrap();
        assert_eq!(silence, [[0.0; 2]; 19]);
        assert_eq!(candidate.position(), destination);
        assert_eq!(candidate.performance_frame(), destination);
        candidate.apply(PlaybackCommand::Play);
    }
    let mut actual = vec![[0.0; 2]; expected.len()];
    for chunk in actual.chunks_mut(127) {
        candidate.render(chunk).unwrap();
    }
    assert_eq!(
        actual, expected,
        "loop continuation from {requested} to {destination}, playing={playing}"
    );
    assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(
        candidate.performance_frame(),
        destination + actual.len() as u64
    );
    assert_eq!(
        candidate.position(),
        if destination < 1024 {
            128 + (destination + actual.len() as u64 - 128) % 896
        } else {
            destination + actual.len() as u64
        }
    );
}

#[inline(never)]
fn assert_preroll_continuation(prepared: &Arc<PreparedRevision>, target: u64, interactive: bool) {
    let mut history = [[0.0; 2]; 129];
    {
        let mut uninterrupted = SessionPlayback::new(Arc::clone(prepared)).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();
        let partitions = [37_usize, 111, 19, 221, 53];
        let mut remaining = target;
        let mut partition_index = 0;
        while remaining != 0 {
            let count =
                remaining.min(partitions[partition_index % partitions.len()] as u64) as usize;
            let mut prefix = vec![[0.0; 2]; count];
            uninterrupted.render(&mut prefix).unwrap();
            remaining -= count as u64;
            partition_index += 1;
        }
        uninterrupted.render(&mut history).unwrap();
    }
    // Linear export reconstructs exact history; interactive seeks start device
    // and voice history empty at the destination like Play-from.
    let expected = if interactive {
        let mut fresh = SessionPlayback::new(Arc::clone(prepared)).unwrap();
        fresh.play_from(target).unwrap();
        let mut expected = [[0.0; 2]; 129];
        fresh.render(&mut expected).unwrap();
        assert_ne!(expected, history, "fixture must expose prefix history");
        expected
    } else {
        history
    };
    let mut candidate = if interactive {
        SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
            Arc::clone(prepared),
            target,
            target,
            true,
            &AtomicBool::new(false),
        )
        .unwrap()
    } else {
        SessionPlayback::new(Arc::clone(prepared))
            .unwrap()
            .prepare_arrangement_preroll(target, target, true, &AtomicBool::new(false))
            .unwrap()
    };
    let mut actual = [[0.0; 2]; 129];
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected, "preroll mismatch at frame {target}");
    assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
}

#[path = "effect_tail_budget_tests.rs"]
mod effect_tail_budget_tests;

#[path = "idle_transition_tests.rs"]
mod idle_transition_tests;

#[path = "return_effect_tail_tests.rs"]
mod return_effect_tail_tests;

#[path = "saturator_tests.rs"]
mod saturator_tests;

#[path = "transition_busy_tests.rs"]
mod transition_busy_tests;

#[path = "eq8_mode_tests.rs"]
mod eq8_mode_tests;

#[path = "flexible_eq_mode_tests.rs"]
mod flexible_eq_mode_tests;

#[path = "compressor_tests.rs"]
mod compressor_tests;

#[path = "filter_tests.rs"]
mod filter_tests;

#[path = "gate_tests.rs"]
mod gate_tests;

#[path = "silence_effect_tests.rs"]
mod silence_effect_tests;

#[path = "source_loop_tests.rs"]
mod source_loop_tests;

#[path = "tests/shared_live_nonlinear.rs"]
mod shared_live_nonlinear;

#[path = "tests/checkpoint_sources.rs"]
mod checkpoint_sources;

#[path = "transport_loop_tests.rs"]
mod transport_loop_tests;

#[path = "pause_release_tests.rs"]
mod pause_release_tests;

#[path = "capture_tests.rs"]
mod capture_tests;

#[path = "device_capacity_tests.rs"]
mod device_capacity_tests;

#[path = "analytic_continuation_tests.rs"]
mod analytic_continuation_tests;

#[path = "instrument_memory_budget_tests.rs"]
mod instrument_memory_budget_tests;

#[path = "harmonic_tests.rs"]
mod harmonic_tests;

#[path = "operator_filter_tests.rs"]
mod operator_filter_tests;

#[path = "mutable_long_tail_tests.rs"]
mod mutable_long_tail_tests;

#[path = "stream_action_tests.rs"]
mod stream_action_tests;

mod cue_isolation;

#[path = "delay_tests.rs"]
mod delay_tests;

#[path = "device_clipboard_voice_tests.rs"]
mod device_clipboard_voice_tests;

#[path = "reverb_bypass_tests.rs"]
mod reverb_bypass_tests;

#[path = "utility_bypass_tests.rs"]
mod utility_bypass_tests;

fn mixed_streamed_resident_and_midi_compensation_case(
    return_gain: Option<f32>,
) -> ([[f32; 2]; 512], u32) {
    #[derive(Default)]
    struct ChangingPages {
        frames: [[f32; 2]; 32],
        reads: Vec<(StreamSourceKey, u64)>,
        requests: Vec<(StreamSourceKey, u64)>,
    }
    impl MediaPages for ChangingPages {
        fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
            self.reads.push((key, page_index));
            (self.reads.len() > 1).then_some(BorrowedMediaPage {
                page_index,
                frames: &self.frames,
            })
        }
        fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.requests.push((key, page_index));
        }
    }
    let source_project = session_audio_midi_arrangement_mix_project();
    let mut editor = Editor::new(source_project, 16).unwrap();
    let track = editor.project().tracks().last().unwrap().id();
    editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "resident".into(),
            path: "resident.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 32,
            playback_frames: 32,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap();
    let limiter = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: limiter.clone(),
        })
        .unwrap();
    if let Some(gain) = return_gain {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "latency return".into(),
            })
            .unwrap()
        else {
            panic!("return track expected");
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
        editor
            .execute(Command::ReturnGain {
                return_track: bus,
                gain,
            })
            .unwrap();
        editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: limiter,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 24,
            enabled: true,
        })
        .unwrap();
    let project = editor.project();
    let media: BTreeMap<_, _> = project
        .assets()
        .map(|asset| {
            let source = if asset.id().value() == 4 {
                PreparedAudioSource::Streamed(Arc::new(
                    StreamedSourceMetadata::new(
                        StreamSourceKey::new(82, asset.id()),
                        asset.source_sample_rate(),
                        project.sample_rate(),
                        asset.source_frames(),
                        asset.playback_frames(),
                        asset.conversion_version(),
                        [0x82; 32],
                    )
                    .unwrap(),
                ))
            } else {
                PreparedAudioSource::Resident(
                    crate::Audio::new(48_000, vec![[0.05, -0.03]; 32]).unwrap(),
                )
            };
            (asset.id(), source)
        })
        .collect();
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            project,
            &media,
            82,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let render = |partition| {
        let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let actual = runtime.accounted_runtime_storage_bytes().unwrap()
            + super::preroll::PREROLL_SCRATCH_BYTES;
        assert!(actual <= preflight, "actual={actual} preflight={preflight}");
        assert_eq!(
            runtime.resident_source_scratch.len(),
            project.tracks().len()
        );
        let clip = revision.session().midi_clips()[0].id;
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pages = ChangingPages {
            frames: [[0.1, -0.2]; 32],
            ..Default::default()
        };
        let mut output = [[0.0; 2]; 512];
        for chunk in output.chunks_mut(partition) {
            runtime.render_with_pages(chunk, &mut pages).unwrap();
        }
        (
            output,
            pages.reads,
            pages.requests,
            runtime.performance_frame(),
            runtime.arrangement_frame(),
        )
    };
    let expected = render(1);
    let actual = render(17);
    assert_eq!(actual.0, expected.0, "batched and scalar PCM must match");
    assert_eq!(
        actual.2, expected.2,
        "batched and scalar page requests must match"
    );
    assert_eq!((actual.3, actual.4), (expected.3, expected.4));
    assert!(actual.0.iter().flatten().any(|sample| *sample != 0.0));
    assert!(
        actual.1.len() > 1 && actual.1.len() <= expected.1.len(),
        "batched compensated routing reads pages in chunks rather than exceeding frame-wise reads"
    );
    assert!(actual.1.iter().all(|key| expected.1.contains(key)));
    assert_eq!(actual.2.len(), 1);
    assert_eq!((actual.3, actual.4), (512, 8));
    (actual.0, revision.arrangement().main_latency_frames())
}

mod instrument_handoff_tests;
mod metering_and_release_tests;
#[path = "mutable_checkpoint_tests.rs"]
mod mutable_checkpoint_tests;
mod mutable_continuity_tests;
mod mutable_handoff_tests;
mod preroll_tests;
mod session_audio_tests;
mod stateful_suffix_tests;
mod tempo_handoff_tests;
