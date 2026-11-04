use super::*;
use crate::routed_midi_fixture;
mod authored_history;
mod cue;
mod instrument_matrix_ramp;
mod interactive_start;
mod release_timeline;
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
                hex_lattice::NativeInstrumentEvent::Expression { .. } => {
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
                    hex_lattice::NativeInstrumentEvent::Expression { .. } => return,
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
            hex_lattice::NativeInstrumentEvent::Expression { .. } => {}
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
            let offset = match event {
                hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::Expression { frame_offset, .. } => {
                    frame_offset
                }
            };
            if offset < previous_offset || offset >= frame_count {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            Self::advance(&mut voices, offset - previous_offset);
            match event {
                hex_lattice::NativeInstrumentEvent::Expression { .. } => {
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
            while events.get(event_index).is_some_and(|event| match *event {
                hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::Expression { frame_offset, .. } => {
                    frame_offset == frame_index
                }
            }) {
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

#[test]
fn wide_analytic_release_and_span_storage_stays_within_derived_fixed_bound() {
    let sampler_bytes = std::mem::size_of::<hex_lattice::PreparedAnalyticNote>();
    assert_eq!(sampler_bytes, 352);
    let added_sampler_bytes = sampler_bytes - 40;
    let release_capacity = super::SESSION_RELEASE_CAPACITY;
    let release_entries_bytes = release_capacity
        .checked_mul(std::mem::size_of::<super::ReleaseEntry>())
        .expect("release entry byte count is bounded")
        .checked_mul(2)
        .expect("paired release entry byte count is bounded");
    let span_buffer_bytes = std::mem::size_of::<MidiSpanBuffer>();
    let current_bytes = release_entries_bytes
        .checked_add(span_buffer_bytes)
        .expect("fixed MIDI storage byte count is bounded");
    let shared_owner_slots = crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS
        .checked_mul(std::mem::size_of::<
            Option<Arc<crate::shared_analytic::SharedContextOwner>>,
        >())
        .expect("shared context slot table is bounded");
    let lease_plan_bytes = crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS
        .checked_mul(std::mem::size_of::<crate::shared_analytic::ContextLeasePlan>())
        .expect("shared context remap plan is bounded");
    let scoped_bytes = current_bytes
        .checked_add(shared_owner_slots)
        .and_then(|bytes| bytes.checked_add(lease_plan_bytes))
        .expect("shared-context playback scratch is bounded");
    assert!(scoped_bytes <= 18 * 1024 * 1024);
    assert!(scoped_bytes.checked_mul(2).unwrap() <= 36 * 1024 * 1024);
    assert_eq!(
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXT_BYTES,
        2 * 1024 * 1024
    );
    assert_eq!(
        crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS,
        SESSION_RELEASE_CAPACITY
    );
    #[cfg(target_pointer_width = "64")]
    {
        // Inline source origin and paused monitor age add 32 bounded bytes.
        // No per-entry allocation; the 18/36 MiB scoped ceilings stay unchanged.
        assert_eq!(std::mem::size_of::<super::ReleaseEntry>(), 528);
        assert_eq!(span_buffer_bytes, 82_952 + 192 * 8);
        assert_eq!(current_bytes, 17_385_992);
        assert_eq!(shared_owner_slots, 131_072);
        assert_eq!(lease_plan_bytes, 393_216);
        assert_eq!(scoped_bytes, 17_910_280);
    }
    let previous_release_entry_size = std::mem::size_of::<super::ReleaseEntry>()
        .checked_sub(added_sampler_bytes)
        .expect("previous sampler fits the entry");
    let previous_span_buffer_size = span_buffer_bytes
        .checked_sub(crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY * added_sampler_bytes)
        .expect("previous sampler fits the span buffer");
    let previous_bytes = release_capacity
        .checked_mul(previous_release_entry_size)
        .and_then(|bytes| bytes.checked_mul(2))
        .and_then(|bytes| bytes.checked_add(previous_span_buffer_size))
        .expect("previous fixed MIDI storage byte count is bounded");
    let expected_growth = release_capacity
        .checked_mul(added_sampler_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .and_then(|bytes| {
            bytes.checked_add(crate::midi_template::MIDI_SPAN_BUFFER_CAPACITY * added_sampler_bytes)
        })
        .expect("fixed MIDI storage growth is bounded");
    assert_eq!(current_bytes - previous_bytes, expected_growth);
    assert!(current_bytes < 17 * 1024 * 1024);
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
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
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
            }],
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
    });
    snapshot.tracks.push(TrackSnapshot {
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
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id: 8,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
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
                }],
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
                        }],
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

#[test]
fn post_node_observation_preserves_pcm_and_exact_loop_render_offsets() {
    use hex_lattice_contract::DeviceTypeId;

    let table = Arc::new(hex_devices::SineTable::new());
    let make_playback = |looping: bool| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 7,
            bypassed: false,
            state,
        });
        snapshot.next_id = snapshot.next_id.max(8);
        if looping {
            snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
        } else {
            snapshot.tempo_points = vec![
                TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
                TempoPoint::new(
                    // The tempo transition falls within the first 256-frame
                    // render chunk; it must not alter the observed render path.
                    BeatTime::from_ratio(1, 100).unwrap(),
                    Tempo::new(90_000_000).unwrap(),
                ),
            ];
        }
        let project = legacy_project_from_snapshot(snapshot).unwrap();
        let prepared = revision(&project, Arc::clone(&table));
        let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
        playback.apply(PlaybackCommand::Play);
        playback
    };

    for (looping, expected_chunks) in [
        (
            true,
            vec![
                (0, 64),
                (64, 64),
                (128, 64),
                (192, 64),
                (256, 64),
                (320, 64),
                (384, 64),
                (448, 64),
            ],
        ),
        (false, vec![(0, 256), (256, 256)]),
    ] {
        let mut ordinary = make_playback(looping);
        let mut observed = make_playback(looping);
        let mut ordinary_output = [[0.0; 2]; 512];
        let mut observed_output = [[0.0; 2]; 512];
        let mut calls = Vec::new();
        ordinary.render(&mut ordinary_output).unwrap();
        observed
            .render_with_post_node_observer(
                &mut observed_output,
                Some(&mut |identity, offset, frames| {
                    if identity.device_type.as_str() == Ok("hex.eq8") {
                        calls.push((identity, offset, frames.len()));
                    }
                }),
            )
            .unwrap();

        assert_eq!(observed_output, ordinary_output);
        let chunks: Vec<_> = calls
            .iter()
            .map(|(_, offset, length)| (*offset, *length))
            .collect();
        assert_eq!(chunks, expected_chunks);
        assert!(calls.iter().all(|(identity, _, _)| {
            identity.owner == hex_lattice::ProcessorChainOwner::Track(1) && identity.device_id == 7
        }));
        let mut paired = make_playback(looping);
        let mut paired_output = [[0.0; 2]; 512];
        let mut paired_calls = Vec::new();
        paired
            .render_with_node_signal_observer(
                &mut paired_output,
                Some(&mut |identity, phase, offset, frames: &[Frame]| {
                    assert_eq!(identity.owner, hex_lattice::ProcessorChainOwner::Track(1));
                    assert_eq!(identity.device_id, 7);
                    paired_calls.push((phase, offset, frames.len()));
                }),
            )
            .unwrap();
        assert_eq!(paired_output, ordinary_output);
        let expected_pairs: Vec<_> = expected_chunks
            .iter()
            .flat_map(|&(offset, count)| {
                [
                    (crate::NodeSignalPhase::Input, offset, count),
                    (crate::NodeSignalPhase::Output, offset, count),
                ]
            })
            .collect();
        assert_eq!(paired_calls, expected_pairs);
    }
}

#[test]
fn post_node_observation_covers_return_chains_bypass_and_session_rendering() {
    use crate::{PlaybackAction, SessionPlaybackCommand};
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{ReturnTrackSnapshot, SendLevelSnapshot};

    let table = Arc::new(hex_devices::SineTable::new());
    let eq8 = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    let make_return_project = |bypassed: bool| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        snapshot.tracks[0].sends.push(SendLevelSnapshot {
            return_track: 70,
            gain: 0.5,
        });
        snapshot.return_tracks.push(ReturnTrackSnapshot {
            id: 70,
            name: "return".into(),
            gain: 1.0,
            muted: false,
            devices: vec![DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: 80,
                bypassed,
                state: eq8.clone(),
            }],
        });
        snapshot.next_id = snapshot.next_id.max(81);
        legacy_project_from_snapshot(snapshot).unwrap()
    };
    let return_identity = crate::PostNodeIdentity::new(
        hex_lattice::ProcessorChainOwner::Return(70),
        80,
        DeviceTypeId::from_static("hex.eq8"),
    );

    let render_return = |bypassed: bool| {
        let project = make_return_project(bypassed);
        let mut playback =
            Playback::try_from_prepared_with_session(revision(&project, Arc::clone(&table)))
                .unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut output = [[0.0; 2]; 256];
        let mut calls = Vec::new();
        playback
            .render_with_post_node_observer(
                &mut output,
                Some(&mut |identity, offset, frames| {
                    calls.push((identity, offset, frames.len()));
                }),
            )
            .unwrap();
        let mut paired =
            Playback::try_from_prepared_with_session(revision(&project, Arc::clone(&table)))
                .unwrap();
        paired.apply(PlaybackCommand::Play);
        let mut paired_output = [[0.0; 2]; 256];
        let mut signals = Vec::new();
        paired
            .render_with_node_signal_observer(
                &mut paired_output,
                Some(&mut |identity, phase, offset, frames: &[Frame]| {
                    if identity == return_identity {
                        signals.push((phase, offset, frames.to_vec()));
                    }
                }),
            )
            .unwrap();
        assert_eq!(paired_output, output);
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].0, crate::NodeSignalPhase::Input);
        assert_eq!(signals[1].0, crate::NodeSignalPhase::Output);
        assert_eq!(signals[0].1, signals[1].1);
        if bypassed {
            assert_eq!(signals[0].2, signals[1].2);
        }
        calls
    };

    let active_return_calls = render_return(false);
    assert!(
        active_return_calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );
    let bypassed_return_calls = render_return(true);
    // A bypassed processor still owns a post-node boundary: the runtime
    // advances its bypass state and exposes the dry signal at that point.
    assert!(
        bypassed_return_calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );

    let project = make_return_project(false);
    let clip = match project.session_cells()[0].content() {
        hex_project::SessionCellContent::Midi(clip) => clip.id(),
        _ => panic!("fixture session cell must be MIDI"),
    };
    let prepared = revision(&project, table);
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    playback
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: crate::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut output = [[0.0; 2]; 256];
    let mut calls = Vec::new();
    playback
        .render_with_post_node_observer(
            &mut output,
            Some(&mut |identity, offset, frames| {
                calls.push((identity, offset, frames.len()));
            }),
        )
        .unwrap();
    assert!(
        calls
            .iter()
            .any(|(identity, _, _)| *identity == return_identity)
    );
}

#[test]
fn removed_track_release_retains_its_original_device_matrix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, DeviceSnapshot, Editor};

    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 10,
        bypassed: false,
        state,
    });
    snapshot.next_id = 11;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut editor = Editor::new(old_project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let mut next = SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(tail.iter().all(|frame| frame[1] == -frame[0]));
}

#[test]
fn ongoing_track_release_keeps_its_original_device_matrix_across_revision() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::DeviceSnapshot;

    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut old_snapshot = base.snapshot();
    old_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let state = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    old_snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: 10,
        bypassed: false,
        state,
    });
    old_snapshot.next_id = 11;
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut new_snapshot = old_project.snapshot();
    let previous_state = &new_snapshot.tracks[0].devices[0].state;
    new_snapshot.tracks[0].devices[0].state = UTILITY_DEFINITION
        .patch_parameter(
            previous_state,
            utility_parameter::INVERT_RIGHT,
            ParameterValue::Bool(false),
        )
        .unwrap();
    let new_project = legacy_project_from_snapshot(new_snapshot).unwrap();
    let mut next = SessionPlayback::new(revision(&new_project, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(tail.iter().all(|frame| frame[1] == -frame[0]));
}

#[test]
fn removed_track_release_runs_only_the_captured_instrument_suffix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, DeviceSnapshot, Editor};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let prefix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let suffix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::CHANNEL,
            ParameterValue::Choice(3), // swap
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 10,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 11,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
    snapshot.next_id = 12;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut editor = Editor::new(old_project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let mut next = SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut tail = [[0.0; 2]; 64];
    next.render(&mut tail).unwrap();
    let audible: Vec<_> = tail
        .iter()
        .filter(|frame| frame[0].abs() > 1.0e-5 || frame[1].abs() > 1.0e-5)
        .collect();
    assert!(!audible.is_empty());
    // The captured suffix swaps stereo. Applying the old prefix would invert
    // only the left channel before that swap and make the channels differ.
    assert!(audible.iter().all(|frame| frame[0] == frame[1]));
}

#[test]
fn same_revision_session_release_enters_at_the_instrument_gap() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::DeviceSnapshot;

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let prefix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let suffix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            utility_parameter::CHANNEL,
            ParameterValue::Choice(3),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 10,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 11,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
    snapshot.next_id = 12;
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut playback = SessionPlayback::new(revision(&project, table)).unwrap();
    let clip = playback.revision.session().midi_clips()[0].id;
    let track = playback.routes[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    playback.render(&mut before).unwrap();
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut tail = [[0.0; 2]; 64];
    playback.render(&mut tail).unwrap();
    let audible: Vec<_> = tail
        .iter()
        .filter(|frame| frame[0].abs() > 1.0e-5 || frame[1].abs() > 1.0e-5)
        .collect();
    assert!(!audible.is_empty());
    // A same-revision release joins after the invert-left prefix, then the
    // current swap suffix runs once. Seeing equal channels catches prefix
    // reprocessing or a second suffix pass.
    assert!(audible.iter().all(|frame| frame[0] == frame[1]));
}

#[test]
fn outgoing_session_release_keeps_the_old_stateful_filter_settings() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state,
        },
    ];
    snapshot.next_id += 2;
    // Put DC removal in the suffix so the outgoing Session voice must keep and
    // execute the captured old stateful processor after the instrument.
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_revision = revision(&old_project, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;

    let render_old_tail = |new_project: &Project| {
        let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();

        let mut next = SessionPlayback::new(revision(new_project, Arc::clone(&table))).unwrap();
        next.try_inherit_state_from(&mut previous).unwrap();
        let mut tail = [[0.0; 2]; 64];
        next.render(&mut tail).unwrap();
        tail
    };

    let stopped = without_session_cells(&old_project);
    let current_dc = with_utility_filter_state_at(&stopped, 1, true, false);
    let current_bass = with_utility_filter_state_at(&stopped, 1, false, true);
    // The candidate route is silent. Its new device configuration must not
    // change outgoing voices, which continue through the captured old chain.
    let old_settings_tail = render_old_tail(&current_dc);
    let new_settings_tail = render_old_tail(&current_bass);
    assert!(
        old_settings_tail
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert_eq!(old_settings_tail, new_settings_tail);
}

#[test]
fn outgoing_arrangement_release_keeps_the_old_stateful_suffix() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_state = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    snapshot.tracks[0].devices = vec![
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state: dc_state,
        },
    ];
    snapshot.next_id += 2;
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_revision = revision(&old_project, Arc::clone(&table));

    let render_old_tail = |new_project: &Project| {
        let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let mut next = SessionPlayback::new(revision(new_project, Arc::clone(&table))).unwrap();
        assert!(!next.revision.arrangement().tracks()[0].instrument_enabled());
        assert!(
            !old_revision.arrangement().tracks()[0]
                .instrument_config_matches(&next.revision.arrangement().tracks()[0])
        );
        next.try_inherit_state_from(&mut previous).unwrap();
        assert!(next.releases.iter().any(|entry| {
            entry.track_id == next.routes[0].id && entry.location.tail_group().is_some()
        }));
        let mut tail = [[0.0; 2]; 64];
        next.render(&mut tail).unwrap();
        tail
    };

    let stopped = without_session_cells(&old_project);
    let mut disabled_snapshot = stopped.snapshot();
    disabled_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    // Keep the current route's own chain output out of the oracle. This leaves
    // the captured release group as the only audible owner after the cut.
    disabled_snapshot.tracks[0].muted = true;
    let disabled = legacy_project_from_snapshot(disabled_snapshot).unwrap();
    let current_dc = with_utility_filter_state_at(&disabled, 1, true, false);
    let current_bass = with_utility_filter_state_at(&disabled, 1, false, true);
    let old_settings_tail = render_old_tail(&current_dc);
    let new_settings_tail = render_old_tail(&current_bass);
    assert!(
        old_settings_tail
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert_eq!(old_settings_tail, new_settings_tail);
}

#[test]
fn detached_arrangement_effect_suffix_renders_after_its_source_ends() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 6,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    snapshot.next_id = 7;
    let source_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_project = with_utility_filter_state(&source_project, true, false);
    let mut bypassed_snapshot = old_project.snapshot();
    bypassed_snapshot.tracks[0].devices[0].bypassed = true;
    let bypassed_project = legacy_project_from_snapshot(bypassed_snapshot).unwrap();
    let changed_configuration = with_utility_filter_state(&old_project, false, true);
    let mut unchanged_snapshot = old_project.snapshot();
    unchanged_snapshot.tracks[0].name = "renamed".into();
    let unchanged_project = legacy_project_from_snapshot(unchanged_snapshot).unwrap();
    let mut changed_mix_snapshot = old_project.snapshot();
    changed_mix_snapshot.tracks[0].gain = 0.25;
    let changed_mix_project = legacy_project_from_snapshot(changed_mix_snapshot).unwrap();

    let media = audio_media(
        &old_project,
        &crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
        &crate::Audio::new(48_000, vec![[0.0, 0.0]; 32]).unwrap(),
    );
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let old_revision = prepare(&old_project);

    // Each transition owns the same two real playback values, but their
    // lifetime is one helper call. Keeping all four pairs in this debug test
    // frame exceeded the ordinary test-thread stack before rendering.
    fn start_old(revision: Arc<PreparedRevision>) -> SessionPlayback {
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 32];
        playback.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));
        assert!(!playback.device_chains[0].committed_range_is_quiescent(0..1));
        playback
    }
    fn reference_output(revision: Arc<PreparedRevision>) -> [Frame; 256] {
        let mut reference = start_old(revision);
        let mut expected = [[0.0; 2]; 256];
        let mut offset = 0;
        for size in [1, 7, 31, 64, 153] {
            reference
                .render(&mut expected[offset..offset + size])
                .unwrap();
            offset += size;
        }
        assert_eq!(offset, expected.len());
        expected
    }
    fn transition_output(
        old: Arc<PreparedRevision>,
        candidate: Arc<PreparedRevision>,
        detached: bool,
    ) -> [Frame; 256] {
        let mut previous = start_old(old);
        let mut current = SessionPlayback::new(candidate).unwrap();
        current.try_inherit_state_from(&mut previous).unwrap();
        assert_eq!(
            current.tail_groups.iter().flatten().count(),
            usize::from(detached)
        );
        if detached {
            assert!(current.tail_output_active[0]);
        }
        let mut output = [[0.0; 2]; 256];
        current.render(&mut output).unwrap();
        output
    }

    let expected = reference_output(Arc::clone(&old_revision));
    // A non-device edit preserves the same owner; history transfers once to
    // the candidate and does not create a second retained old suffix.
    assert_eq!(
        transition_output(
            Arc::clone(&old_revision),
            prepare(&unchanged_project),
            false
        ),
        expected
    );
    // Bypass detaches the old output settings.
    assert_eq!(
        transition_output(Arc::clone(&old_revision), prepare(&bypassed_project), true),
        expected
    );
    // A compatible runtime inherits filter history for outgoing residual audio
    // without also copying that old-configuration history into the new chain.
    assert_eq!(
        transition_output(
            Arc::clone(&old_revision),
            prepare(&changed_configuration),
            true
        ),
        expected
    );
    // Mix-only edits keep the same filter history under the new gain. They
    // must not capture an independently audible old-mix suffix.
    assert_eq!(
        transition_output(old_revision, prepare(&changed_mix_project), false),
        expected.map(|frame| [frame[0] * 0.25, frame[1] * 0.25])
    );
}

#[test]
fn eq8_parameter_edits_inherit_arrangement_effect_history_without_retaining_old_suffix() {
    use hex_devices::eq8::parameter as eq8_parameter;
    use hex_lattice::NativeDeviceDefinition;
    use hex_lattice_contract::ParameterValue;

    let table = Arc::new(hex_devices::SineTable::new());
    let make_project = |gain_db: f32, q: f32, kind: u32| {
        let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
        let mut state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        for parameter in [eq8_parameter::enabled(0, 0), eq8_parameter::enabled(1, 0)] {
            state =
                crate::patch_native_device_parameter(&state, parameter, ParameterValue::Bool(true))
                    .unwrap();
        }
        for (parameter, value) in [
            (eq8_parameter::gain(0, 0), ParameterValue::Float(gain_db)),
            (eq8_parameter::q(0, 0), ParameterValue::Float(q)),
            (eq8_parameter::kind(0, 0), ParameterValue::Choice(kind)),
        ] {
            state = crate::patch_native_device_parameter(&state, parameter, value).unwrap();
        }
        snapshot.tracks[0].devices.push(DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: 7,
            bypassed: false,
            state: state.clone(),
        });
        snapshot.next_id = snapshot.next_id.max(8);
        (legacy_project_from_snapshot(snapshot).unwrap(), state)
    };
    let mut source_snapshot = project(0.5, 0, 8 << 32).snapshot();
    source_snapshot.master_gain = 1.0;
    let source_project = legacy_project_from_snapshot(source_snapshot).unwrap();
    let mut source = SessionPlayback::new(revision(&source_project, Arc::clone(&table))).unwrap();
    source.apply(SessionPlaybackCommand::Play).unwrap();

    let (old_project, old_state) = make_project(3.0, 1.0, 0);
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let old_prepared = hex_devices::eq8::EQ8_DEFINITION
        .prepare_processor(
            &old_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let mut reference_runtime = old_prepared.create_runtime();
    let mut input = [[0.0; 2]; 128];
    let mut expected = input;
    let mut output = input;
    source.render(&mut input).unwrap();
    expected.copy_from_slice(&input);
    reference_runtime.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    old.render(&mut output).unwrap();
    assert_eq!(output, expected, "initial registered EQ8 PCM oracle");
    assert!(!old.device_chains[0].committed_range_is_quiescent(0..1));

    for (gain_db, q, kind) in [(6.0, 1.0, 0), (6.0, 2.0, 0), (6.0, 2.0, 1)] {
        let (candidate_project, candidate_state) = make_project(gain_db, q, kind);
        let mut candidate =
            SessionPlayback::new(revision(&candidate_project, Arc::clone(&table))).unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(candidate.tail_groups.iter().flatten().count(), 0);
        assert!(!candidate.device_chains[0].committed_range_is_quiescent(0..1));
        let prepared = hex_devices::eq8::EQ8_DEFINITION
            .prepare_processor(
                &candidate_state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let mut next_reference = prepared.create_runtime();
        assert!(next_reference.can_retarget_parameters_from(reference_runtime.as_ref()));
        next_reference.copy_history_from(reference_runtime.as_ref());
        input.fill([0.0; 2]);
        expected.fill([0.0; 2]);
        output.fill([0.0; 2]);
        source.render(&mut input).unwrap();
        expected.copy_from_slice(&input);
        next_reference.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        candidate.render(&mut output).unwrap();
        assert_eq!(output, expected, "retargeted EQ8 parameter PCM oracle");
        assert!(
            output
                .iter()
                .all(|frame| frame.iter().all(|sample| sample.is_finite()))
        );
        assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-6));
        reference_runtime = next_reference;
        old = candidate;
    }
}

#[test]
fn removing_track_keeps_its_unquiescent_arrangement_effect_suffix() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 6,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    snapshot.next_id = 7;
    let project = with_utility_filter_state(
        &legacy_project_from_snapshot(snapshot).unwrap(),
        true,
        false,
    );
    let mut media = BTreeMap::new();
    let asset_id = project.assets().next().unwrap().id();
    media.insert(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
    );
    let table_for_revision = Arc::clone(&table);
    let mut old = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(&project, &media, table_for_revision, None).unwrap(),
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    old.render(&mut prefix).unwrap();
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let mut expected = [[0.0; 2]; 256];
    old.render(&mut expected).unwrap();
    assert!(expected.iter().any(|frame| frame[0].abs() > 1.0e-6));

    let mut previous = SessionPlayback::new(Arc::clone(&old.revision)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut previous_prefix = [[0.0; 2]; 32];
    previous.render(&mut previous_prefix).unwrap();

    let mut editor = Editor::new(project, 8).unwrap();
    let track_id = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track_id)).unwrap();
    let candidate_revision =
        Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.tail_groups.iter().flatten().count(), 1);
    let mut actual = [[0.0; 2]; 256];
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn utility_mute_handoff_silences_warmed_filter_without_retaining_an_audible_tail() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    for (on_return, dc, bass, audition) in [
        (false, true, false, false),
        (false, false, true, false),
        (false, false, true, true),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, true, true),
    ] {
        let mut snapshot = audio_project(0, 32, 4).snapshot();
        let mut state = UTILITY_DEFINITION.default_state().unwrap();
        for (parameter, value) in [
            (parameter::DC_REMOVE, dc),
            (parameter::BASS_MONO, bass),
            (parameter::BASS_MONO_AUDITION, audition),
        ] {
            state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter, ParameterValue::Bool(value))
                .unwrap();
        }
        let device = DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: state.clone(),
        };
        snapshot.next_id += 1;
        if on_return {
            let return_id = snapshot.next_id;
            snapshot.next_id += 1;
            snapshot.tracks[0]
                .sends
                .push(hex_project::SendLevelSnapshot {
                    return_track: return_id,
                    gain: 1.0,
                });
            snapshot
                .return_tracks
                .push(hex_project::ReturnTrackSnapshot {
                    id: return_id,
                    name: "filtered return".into(),
                    gain: 1.0,
                    muted: false,
                    devices: vec![device],
                });
        } else {
            snapshot.tracks[0].devices.push(device);
        }
        let project = legacy_project_from_snapshot(snapshot).unwrap();
        let table = Arc::new(hex_devices::SineTable::new());
        let asset = project
            .assets()
            .find(|asset| asset.id().value() == 4)
            .unwrap()
            .id();
        let media = BTreeMap::from([(
            asset,
            crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
        )]);
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let revision = prepare(&project);
        let clip = revision.session().audio_clips()[0].id;
        let mut previous = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for playback in [&mut previous, &mut reference] {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut warmup = [[0.0; 2]; 32];
            playback.render(&mut warmup).unwrap();
            assert!(warmup.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        }

        let mut muted_snapshot = project.snapshot();
        let devices = if on_return {
            &mut muted_snapshot.return_tracks[0].devices
        } else {
            &mut muted_snapshot.tracks[0].devices
        };
        devices[0].state = UTILITY_DEFINITION
            .patch_parameter(&state, parameter::MUTE, ParameterValue::Bool(true))
            .unwrap();
        let muted_project = legacy_project_from_snapshot(muted_snapshot).unwrap();
        let mut muted = SessionPlayback::new(prepare(&muted_project)).unwrap();
        muted.try_inherit_state_from(&mut previous).unwrap();
        let mut output = [[1.0; 2]; 16];
        muted.render(&mut output).unwrap();
        assert_eq!(
            output, [[0.0; 2]; 16],
            "mute retained an audible old filter for return={on_return}, dc={dc}, bass={bass}, audition={audition}"
        );
        assert!(muted.tail_groups.iter().all(Option::is_none));
        // The reference's source has ended, so both filters advance through
        // exactly sixteen zero-input frames. Unmute must recover that history.
        reference.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut unmuted = SessionPlayback::new(prepare(&project)).unwrap();
        unmuted.try_inherit_state_from(&mut muted).unwrap();
        let mut expected = [[0.0; 2]; 16];
        reference.render(&mut expected).unwrap();
        unmuted.render(&mut output).unwrap();
        assert_eq!(output, expected);
    }
}

#[test]
fn compatible_stateful_history_survives_crossing_the_instrument_gap() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::Command;

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    let instrument_state = hex_project::default_instrument_state().unwrap();
    let instrument_id = snapshot.next_id;
    snapshot.next_id += 1;
    let device_id = snapshot.next_id;
    snapshot.next_id += 1;
    let dc_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].instrument = Some(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: instrument_id,
        bypassed: false,
        state: instrument_state,
    });
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: device_id,
        bypassed: false,
        state: dc_state,
    }];
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let track_id = old_project.tracks()[0].id();
    let instrument_id = old_project.tracks()[0].instrument().unwrap().id();
    let device_id = old_project.tracks()[0].devices()[0].id();
    let asset_id = old_project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap(),
    )]);
    let old_revision = Arc::new(
        PreparedRevision::prepare(&old_project, &media, Arc::clone(&table), None).unwrap(),
    );
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let clip = old_revision.session().audio_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup = [[0.0; 2]; 16];
    let mut reference_warmup = [[0.0; 2]; 16];
    let mut reference = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    reference
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut reference_warmup).unwrap();
    previous.render(&mut warmup).unwrap();
    assert_eq!(warmup, reference_warmup);

    let mut editor = Editor::new(old_project, 8).unwrap();
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument_id,
            track: track_id,
            before: Some(device_id),
        })
        .unwrap();
    let candidate_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            table,
            Some(old_revision.session()),
        )
        .unwrap(),
    );
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();

    let mut expected = [[0.0; 2]; 16];
    let mut actual = [[0.0; 2]; 16];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
}

#[test]
fn stateful_prefix_and_suffix_match_the_finite_arrangement_across_session_blocks() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: state.clone(),
        },
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id + 1,
            bypassed: false,
            state,
        },
    ];
    snapshot.next_id += 2;
    snapshot.tracks[0].instrument_position = Some(1);
    let prepared = revision(&legacy_project_from_snapshot(snapshot).unwrap(), table);
    let clip = prepared.session().midi_clips()[0].id;
    let mut finite = Playback::from_prepared(Arc::clone(&prepared));
    let mut session = SessionPlayback::new(prepared).unwrap();
    finite.apply(PlaybackCommand::Play);
    session
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();

    let mut expected = [[0.0; 2]; 12_000];
    let mut actual = [[0.0; 2]; 12_000];
    finite.render(&mut expected).unwrap();
    let mut offset = 0;
    for count in [1, 17, 127, 256, 31, 1024, 4096, 6448] {
        session.render(&mut actual[offset..offset + count]).unwrap();
        offset += count;
    }
    assert_eq!(offset, actual.len());
    assert_eq!(actual, expected);
}

#[test]
fn active_tail_group_moves_with_its_unquiescent_suffix_across_revision() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_utility_filter_state(&project(0.5, 0, 8 << 32), true, false);
    let base_revision = revision(&base, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&base_revision)).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let changed = with_utility_filter_state(
        &project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap()),
        true,
        false,
    );
    let mut first = SessionPlayback::new(revision(&changed, Arc::clone(&table))).unwrap();
    first.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(first.tail_groups.iter().flatten().count(), 1);

    // This suffix is still producing residual output, so the next candidate
    // must inherit its group and continue advancing it. A quiescent group may
    // remain on the old owner for control-side retirement instead.
    let changed_again = with_utility_filter_state(&base, true, false);
    let mut second = SessionPlayback::new(revision(&changed_again, table)).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    assert!(first.tail_groups[0].is_none());
    assert_eq!(second.tail_groups[0].as_ref().unwrap().stateful_count(), 1);
    assert_eq!(second.tail_groups.iter().flatten().count(), 1);
    assert!(second.tail_output_active[0]);
}

#[test]
fn full_stateful_tail_bank_refuses_before_mutating_the_live_graph() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_utility_filter_state(&project(0.5, 0, 8 << 32), true, false);
    let base_revision = revision(&base, Arc::clone(&table));
    let changed = with_utility_filter_state(
        &project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap()),
        true,
        false,
    );
    let clip_id = base_revision.session().midi_clips()[0].id;

    let make_full = || {
        let mut previous = SessionPlayback::new(Arc::clone(&base_revision)).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: clip_id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();

        let mut active = SessionPlayback::new(revision(&changed, Arc::clone(&table))).unwrap();
        active.try_inherit_state_from(&mut previous).unwrap();
        let mut next_prefix = [[0.0; 2]; 64];
        active.render(&mut next_prefix).unwrap();

        let release = *active
            .releases
            .first()
            .expect("first handoff captures a release");
        for slot in 1..SESSION_TAIL_GROUP_CAPACITY {
            active.tail_groups[slot] = Some(hex_lattice::NativeRuntimeChain::empty());
            let mut grouped = release;
            grouped.location = ReleaseLocation::new(
                grouped.location.track_index(),
                Some(u16::try_from(slot).unwrap()),
            )
            .unwrap();
            active.releases.push(grouped);
        }
        let mut current_chain_release = release;
        // This extra voice belongs to the current source chain, unlike the
        // detached historical group copied above. Give it explicit custody.
        current_chain_release.source_origin = ReleaseSourceOrigin::capture(&active.revision, 0);
        current_chain_release.location = ReleaseLocation::new(0, None).unwrap();
        active.releases.push(current_chain_release);
        active
    };

    let mut active = make_full();
    let mut reference = make_full();
    assert_eq!(
        active.tail_groups.iter().flatten().count(),
        SESSION_TAIL_GROUP_CAPACITY
    );
    let next_project = with_utility_filter_state(&base, true, false);
    let mut candidate = SessionPlayback::new(revision(&next_project, Arc::clone(&table))).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut active),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(
        active.tail_groups.iter().flatten().count(),
        SESSION_TAIL_GROUP_CAPACITY
    );
    assert_eq!(active.device_chains[0].stateful_count(), 1);

    let mut after_refusal = [[0.0; 2]; 128];
    let mut reference_output = [[0.0; 2]; 128];
    active.render(&mut after_refusal).unwrap();
    reference.render(&mut reference_output).unwrap();
    assert_eq!(after_refusal, reference_output);
}

#[test]
fn stateful_filter_releases_clip_history_on_pause_resets_on_seek_stop_and_continues_across_loop() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut base_snapshot = project(0.5, 0, 8 << 32).snapshot();
    base_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let base = with_utility_filter_state(
        &legacy_project_from_snapshot(base_snapshot).unwrap(),
        true,
        false,
    );

    let start = |revision: Arc<PreparedRevision>, clip: hex_project::ClipId| {
        let mut playback = SessionPlayback::new(revision).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };

    let prepared = revision(&base, Arc::clone(&table));
    let clip = prepared.session().midi_clips()[0].id;
    let mut paused = start(Arc::clone(&prepared), clip);
    let mut prefix = [[0.0; 2]; 32];
    paused.render(&mut prefix).unwrap();
    let frame = paused.performance_frame();
    let mut released = start(Arc::clone(&prepared), clip);
    released.render(&mut [[0.0; 2]; 32]).unwrap();
    released
        .apply(SessionPlaybackCommand::StopTrack {
            track: base.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused_release = [[0.0; 2]; 64];
    let mut expected_release = paused_release;
    paused.render(&mut paused_release).unwrap();
    released.render(&mut expected_release).unwrap();
    assert_eq!(
        paused_release, expected_release,
        "pause releases through the existing filter history"
    );
    assert_eq!(paused.performance_frame(), frame);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_pause = [[0.0; 2]; 64];
    let mut expected_tail = after_pause;
    paused.render(&mut after_pause).unwrap();
    released.render(&mut expected_tail).unwrap();
    assert_eq!(
        after_pause, expected_tail,
        "resume does not retrigger the paused clip note"
    );

    let mut sought = start(Arc::clone(&prepared), clip);
    let mut pre_seek = [[0.0; 2]; 32];
    sought.render(&mut pre_seek).unwrap();
    sought.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    sought.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_seek = [[0.0; 2]; 64];
    sought.render(&mut after_seek).unwrap();
    let mut fresh_seek = start(Arc::clone(&prepared), clip);
    fresh_seek.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    fresh_seek.apply(SessionPlaybackCommand::Play).unwrap();
    let mut fresh_seek_output = [[0.0; 2]; 64];
    fresh_seek.render(&mut fresh_seek_output).unwrap();
    assert_eq!(after_seek, fresh_seek_output);

    let mut stopped = start(Arc::clone(&prepared), clip);
    let mut pre_stop = [[0.0; 2]; 32];
    stopped.render(&mut pre_stop).unwrap();
    stopped.apply(SessionPlaybackCommand::Stop).unwrap();
    stopped.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_stop = [[0.0; 2]; 64];
    stopped.render(&mut after_stop).unwrap();
    let mut fresh_stop = start(Arc::clone(&prepared), clip);
    fresh_stop.apply(SessionPlaybackCommand::Stop).unwrap();
    fresh_stop.apply(SessionPlaybackCommand::Play).unwrap();
    let mut fresh_stop_output = [[0.0; 2]; 64];
    fresh_stop.render(&mut fresh_stop_output).unwrap();
    assert_eq!(after_stop, fresh_stop_output);

    let mut loop_snapshot = base.snapshot();
    loop_snapshot.loop_region = Some(LoopRegion::new(0, 64, true).unwrap());
    let looped = legacy_project_from_snapshot(loop_snapshot).unwrap();
    let mut with_loop = start(revision(&looped, Arc::clone(&table)), clip);
    let mut without_loop = start(prepared, clip);
    let mut looped_output = [[0.0; 2]; 512];
    let mut plain_output = [[0.0; 2]; 512];
    with_loop.render(&mut looped_output).unwrap();
    without_loop.render(&mut plain_output).unwrap();
    assert_eq!(looped_output, plain_output);
}

#[test]
fn session_render_failure_after_stateful_prefix_rolls_back_filter_history_and_clock() {
    let table = Arc::new(hex_devices::SineTable::new());
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mut snapshot = project(0.5, 0, 8 << 32).snapshot();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut devices = Vec::new();
    for _ in 0..2 {
        let id = snapshot.next_id;
        snapshot.next_id += 1;
        devices.push(hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id,
            bypassed: false,
            state: state.clone(),
        });
    }
    snapshot.tracks[0].devices = devices;
    snapshot.tracks[0].instrument_position = Some(1);
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    let prepared = revision(&project, table);
    let clip = prepared.session().midi_clips()[0].id;
    let mut failed = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut reference = SessionPlayback::new(prepared).unwrap();
    for playback in [&mut failed, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    failed.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    failed.release_scratch.clear();
    let owner = failed.routes[0].owner;
    failed
        .capture_outgoing(0, owner, failed.arrangement_frame, failed.performance_frame)
        .unwrap();
    assert!(!failed.release_scratch.is_empty());
    let mut invalid_tail = failed.release_scratch[0];
    invalid_tail.location = ReleaseLocation::corrupt_detached_tail_for_test();
    failed.release_scratch.clear();
    failed.release_scratch.push(invalid_tail);
    std::mem::swap(&mut failed.releases, &mut failed.release_scratch);

    let committed_frame = failed.performance_frame;
    let mut refused = [[1.0; 2]; 256];
    assert_eq!(
        failed.render(&mut refused),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(refused, [[0.0; 2]; 256]);
    assert_eq!(failed.performance_frame, committed_frame);
    assert_eq!(failed.arrangement_frame, reference.arrangement_frame);

    // Remove only the deliberately corrupt release, then verify the retry has
    // the exact same processor history and clock as the untouched twin.
    failed.releases.clear();
    let mut retry = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    failed.render(&mut retry).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(retry, expected);
    assert_eq!(failed.performance_frame, reference.performance_frame);
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

#[test]
fn ordinary_fresh_mutable_install_suppresses_precut_gates_with_atomic_capacity_refusal() {
    use hex_lattice::NativeInstrumentDefinition;
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.fresh-install"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let enabled = with_test_instrument(
        &with_second_arrangement_note(&project(1.0, 0, 8 << 32)),
        definition.metadata.id,
        0,
    );
    let track = enabled.tracks()[0].id();
    let device = enabled.tracks()[0].instrument().unwrap().id();
    let mut editor = Editor::new(enabled.clone(), 8).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let disabled = editor.project().clone();
    for session in [false, true] {
        let mut old = ordinary_mutable_playback(&disabled, &registry);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip: old.revision.session().midi_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.render(&mut [[0.0; 2]; 64]).unwrap();
        let old_owner = old.owner(track);
        let mut candidate = ordinary_mutable_playback(&enabled, &registry);
        candidate.release_capacity = 0;
        assert_eq!(
            candidate.try_inherit_state_from(&mut old),
            Err(SessionPlaybackError::ReleaseBankFull)
        );
        assert_eq!(old.performance_frame(), 64);
        assert_eq!(old.owner(track), old_owner);
        assert!(old.suppressions.is_empty());
        assert_eq!(candidate.performance_frame(), 0);
        assert!(candidate.suppressions.is_empty());
        candidate.release_capacity = SESSION_RELEASE_CAPACITY;
        candidate.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(candidate.suppressions.len(), 1);
        let mut silent = [[1.0; 2]; 64];
        candidate.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 64]);
        // The one-shot exception is no longer present on an unchanged edit, but its exact
        // suppression remains until the original occurrence ends.
        let mut successor = ordinary_mutable_playback(&enabled, &registry);
        successor.try_inherit_state_from(&mut candidate).unwrap();
        successor.render(&mut silent).unwrap();
        assert_eq!(silent, [[0.0; 2]; 64]);
        if !session {
            let mut until_next = vec![[0.0; 2]; 24_000 - 192];
            successor.render(&mut until_next).unwrap();
            assert!(until_next.iter().all(|frame| *frame == [0.0; 2]));
            successor.render(&mut silent[..1]).unwrap();
            assert!(silent[0][0] > 0.0, "exact future onset is dispatched");
        }
    }
    // Complete old evidence also distinguishes a genuinely new canonical track from an
    // unobserved predecessor. Its pre-cut gate is suppressed without altering old sources.
    let mut new_track_editor = Editor::new(disabled.clone(), 8).unwrap();
    let Created::Track(new_track) = new_track_editor
        .execute(Command::AddMidiTrack { name: "new".into() })
        .unwrap()
    else {
        panic!("track")
    };
    new_track_editor
        .execute(Command::InsertInstrument {
            track: new_track,
            state: hex_lattice_contract::NativeDeviceState::new(definition.metadata.id, 1, vec![0])
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(new_clip) = new_track_editor
        .execute(Command::PlaceMidiClip {
            track: new_track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    new_track_editor
        .execute(Command::AddMidiNote {
            clip: new_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut old = ordinary_mutable_playback(&disabled, &registry);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut new_track_candidate = ordinary_mutable_playback(new_track_editor.project(), &registry);
    new_track_candidate
        .try_inherit_state_from(&mut old)
        .unwrap();
    let mut silent = [[1.0; 2]; 64];
    new_track_candidate.render(&mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 64]);

    // A changed active MIDI signature is not an installation proof, even alongside a real
    // authored device discontinuity. Missing legacy evidence also cannot authorize suppression.
    let mut old = ordinary_mutable_playback(&disabled, &registry);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut changed = Editor::new(enabled.clone(), 8).unwrap();
    changed
        .execute(Command::TrimMidiClipEdge {
            clip: enabled.tracks()[0].midi_clips()[0].id(),
            edge: hex_project::TrimEdge::Right,
            beat: BeatTime::from_raw(2 << 32),
        })
        .unwrap();
    let mut missing_source_proof = ordinary_mutable_playback(changed.project(), &registry);
    assert_eq!(
        missing_source_proof.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(old.performance_frame(), 64);
    let mut legacy = SessionPlayback::new(revision_with_instrument_registry(
        &disabled,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    ))
    .unwrap();
    legacy.apply(SessionPlaybackCommand::Play).unwrap();
    legacy.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        ordinary_mutable_playback(&enabled, &registry).try_inherit_state_from(&mut legacy),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
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

fn render_checkpoint_partitioned(
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

#[test]
fn shared_context_table_is_common_to_arrangement_session_and_reuses_exact_configuration() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.shared.context");
    let definition = ContinuityInstrumentDefinition {
        id,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let original = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let arrangement_contexts = original.arrangement().shared_contexts();
    let session_contexts = original.session().shared_contexts();
    assert!(Arc::ptr_eq(arrangement_contexts, session_contexts));
    assert!(arrangement_contexts.storage_bytes().unwrap() > 0);
    assert_eq!(arrangement_contexts.payload_bytes(), 16);
    let device = base.tracks()[0].instrument().unwrap().id();
    let index = arrangement_contexts.index_for_device(device).unwrap();
    let original_id = arrangement_contexts.owner(index).unwrap().id();
    assert_eq!(arrangement_contexts.index_for_id(original_id), Some(index));

    let compatible = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &base,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original.session()),
            &registry,
        )
        .unwrap(),
    );
    assert_eq!(
        compatible
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );
    assert!(Arc::ptr_eq(
        compatible.arrangement().shared_contexts(),
        compatible.session().shared_contexts()
    ));

    // Context identity belongs to the retained source. Moving its receiver
    // while keeping the immutable configuration reuses the same context.
    let mut moved_snapshot = base.snapshot();
    moved_snapshot.tracks[0].id = moved_snapshot.next_id;
    moved_snapshot.next_id += 1;
    moved_snapshot.session = Default::default();
    let moved = Project::from_legacy_snapshot(moved_snapshot).unwrap();
    let moved = PreparedRevision::prepare_with_instrument_registry(
        &moved,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(original.session()),
        &registry,
    )
    .unwrap();
    assert_eq!(
        moved
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );

    let changed = with_test_instrument(&base, id, 1);
    let changed = PreparedRevision::prepare_with_instrument_registry(
        &changed,
        &BTreeMap::new(),
        table,
        Some(original.session()),
        &registry,
    )
    .unwrap();
    assert_ne!(
        changed
            .arrangement()
            .shared_contexts()
            .owner_for_device(device)
            .unwrap()
            .id(),
        original_id
    );
}

#[test]
fn shared_context_table_admits_the_full_track_bound_without_duplicate_owners() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.shared.context");
    let definition = ContinuityInstrumentDefinition {
        id,
        levels: [0.25, 0.5],
        release_frames: 4,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
    for index in 0..hex_project::MAX_TRACKS {
        let track = match editor
            .execute(Command::InsertTrackWithDevice {
                name: format!("context {index}"),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        {
            Created::Track(track) => track,
            _ => panic!("MIDI track command returned an unexpected value"),
        };
        let device = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap(),
            })
            .unwrap();
    }
    let prepared = revision_with_instrument_registry(editor.project(), table, &registry);
    let contexts = prepared.arrangement().shared_contexts();
    assert_eq!(contexts.context_count(), hex_project::MAX_TRACKS);
    assert_eq!(contexts.payload_bytes(), 16 * hex_project::MAX_TRACKS);
    assert_eq!(
        contexts.storage_bytes().unwrap(),
        std::mem::size_of_val(contexts.as_ref())
            + hex_project::MAX_TRACKS * std::mem::size_of::<(TrackId, hex_project::DeviceId)>()
            + contexts.context_count()
                * (std::mem::size_of::<Arc<crate::shared_analytic::SharedContextOwner>>()
                    + std::mem::size_of::<(crate::shared_analytic::SharedContextId, u16,)>())
    );
}

#[test]
fn shared_context_preparation_refuses_payload_above_per_context_limit() {
    let definition = OversizedSharedContextDefinition;
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let source = with_test_instrument(
        &project(0.5, 0, 8 << 32),
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.oversized-context"),
        0,
    );
    let result = PreparedRevision::prepare_with_instrument_registry(
        &source,
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
        &registry,
    );
    assert_eq!(
        result.unwrap_err().0,
        "shared analytic context exceeds its byte limit"
    );
}

#[test]
fn registered_mutable_instrument_adds_feedback_audio_in_arrangement_and_session() {
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
    let base = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);

    for session_owner in [false, true] {
        let source = if session_owner {
            session_only_project(&base)
        } else {
            base.clone()
        };
        let revision = revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
        let mut whole = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut partitioned = SessionPlayback::new(revision).unwrap();
        if session_owner {
            let clip = whole.revision.session().midi_clips()[0].id;
            for playback in [&mut whole, &mut partitioned] {
                playback
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                playback.apply(SessionPlaybackCommand::Play).unwrap();
            }
        } else {
            whole.apply(SessionPlaybackCommand::Play).unwrap();
            partitioned.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut expected = [[0.0_f32; 2]; 16];
        let mut feedback = 0.0_f32;
        for frame in &mut expected {
            let direct = 0.2_f32;
            feedback = feedback * 0.5 + direct * 0.25;
            frame[0] = direct + feedback;
            frame[1] = direct + feedback;
        }
        let mut actual = [[0.0; 2]; 16];
        whole.render(&mut actual).unwrap();
        assert_eq!(
            actual, expected,
            "mutable runtime must add its independent feedback law once"
        );
        let mut first = [[0.0; 2]; 5];
        let mut rest = [[0.0; 2]; 11];
        partitioned.render(&mut first).unwrap();
        partitioned.render(&mut rest).unwrap();
        assert_eq!(first.as_slice(), &expected[..5]);
        assert_eq!(rest.as_slice(), &expected[5..]);
    }
}

#[test]
fn prepared_revision_budget_counts_return_effects_with_mutable_instruments_transactionally() {
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
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let filter_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut snapshot = source.snapshot();
    let mut next_id = snapshot.next_id;
    let mut return_tracks = Vec::new();
    for return_index in 0..hex_project::MAX_RETURN_TRACKS {
        let devices_in_chain = if return_index + 1 == hex_project::MAX_RETURN_TRACKS {
            15
        } else {
            16
        };
        let mut devices = Vec::new();
        for _ in 0..devices_in_chain {
            devices.push(DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: next_id,
                bypassed: false,
                state: filter_state.clone(),
            });
            next_id += 1;
        }
        return_tracks.push(hex_project::ReturnTrackSnapshot {
            id: next_id,
            name: format!("return {return_index}"),
            gain: 1.0,
            muted: false,
            devices,
        });
        next_id += 1;
    }
    snapshot.next_id = next_id;
    snapshot.return_tracks = return_tracks;
    let base = Project::from_legacy_snapshot(snapshot).unwrap();
    let return_track = base.return_tracks().last().unwrap().id();
    let mut editor = Editor::new(base, 16).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    // Exactly 255 stateful Returns plus one mutable instrument are admitted.
    // The candidate below adds the 256th Return effect and exceeds the joint cap.
    let predecessor = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut predecessor_playback = SessionPlayback::new(Arc::clone(&predecessor)).unwrap();
    let mut oracle_playback = SessionPlayback::new(Arc::clone(&predecessor)).unwrap();
    predecessor_playback
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    oracle_playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 16];
    predecessor_playback.render(&mut prefix).unwrap();
    oracle_playback.render(&mut prefix).unwrap();

    let before = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let can_redo = editor.can_redo();
    let rejected = editor.execute_prepared(
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: filter_state,
        },
        |candidate| {
            PreparedRevision::prepare_with_instrument_registry(
                candidate,
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(predecessor.session()),
                &registry,
            )
            .map_err(|_| hex_project::Invalid("prepared graph exceeded runtime budget"))
        },
    );
    assert_eq!(
        rejected.unwrap_err().0,
        "prepared graph exceeded runtime budget"
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo);
    assert_eq!(editor.can_redo(), can_redo);

    let mut after_rejection = [[0.0; 2]; 16];
    let mut oracle_after_rejection = [[0.0; 2]; 16];
    predecessor_playback.render(&mut after_rejection).unwrap();
    oracle_playback.render(&mut oracle_after_rejection).unwrap();
    assert_eq!(after_rejection, oracle_after_rejection);
}

#[test]
fn unchanged_mutable_arrangement_and_session_owners_keep_history_across_editor_revisions() {
    use hex_project::{Command, Editor};

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

    for (session_owner, note_duration, recent_release) in [
        (false, 8 << 32, false),
        (true, 8 << 32, false),
        (false, (1 << 32) / 6_000, true),
        (true, (1 << 32) / 6_000, true),
    ] {
        let base = with_test_instrument(&project(1.0, 0, note_duration), id, 0);
        let source = if session_owner {
            session_only_project(&base)
        } else {
            base
        };
        let initial_revision =
            revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
        let mut playback = SessionPlayback::new(Arc::clone(&initial_revision)).unwrap();
        let mut uninterrupted = SessionPlayback::new(initial_revision).unwrap();
        if session_owner {
            let clip = playback.revision.session().midi_clips()[0].id;
            for owner in [&mut playback, &mut uninterrupted] {
                owner
                    .apply(SessionPlaybackCommand::LaunchClip {
                        clip,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
            }
        }
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();

        let mut preroll_frames = 0;
        loop {
            let mut actual = [[0.0; 2]; 1];
            let mut expected = [[0.0; 2]; 1];
            playback.render(&mut actual).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            preroll_frames += 1;
            let runtime = playback.mutable_instruments[0].as_ref().unwrap();
            if recent_release {
                if runtime.committed_feedback_tail_is_active() {
                    break;
                }
            } else if preroll_frames == 16 {
                assert!(!runtime.committed_ledger_is_empty());
                break;
            }
            assert!(
                preroll_frames < 64,
                "fixture did not reach its test boundary"
            );
        }

        let mut editor = Editor::new(source, 8).unwrap();
        for name in ["first unrelated edit", "second unrelated edit"] {
            let track = editor.project().tracks()[0].id();
            let (_, candidate_revision) = editor
                .execute_prepared(
                    Command::RenameTrack {
                        track,
                        name: name.to_owned(),
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
            let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
            candidate.try_inherit_state_from(&mut playback).unwrap();
            assert!(
                playback
                    .tail_mutable_instruments
                    .iter()
                    .all(Option::is_none)
            );
            assert_eq!(
                candidate.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .next_occurrence_id(),
                uninterrupted.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .next_occurrence_id()
            );
            let mut actual = [[0.0; 2]; 24];
            let mut expected = [[0.0; 2]; 24];
            candidate.render(&mut actual).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(actual, expected);
            playback = candidate;
        }
    }
}

#[test]
fn unchanged_mutable_session_route_carries_across_another_route_source_edit() {
    use hex_lattice::NativeInstrumentDefinition;

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "short source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let second_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "held source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected second MIDI track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected Session scene"),
    };
    let mut first_note = None;
    for (track, pitch, duration) in [
        (first_track, 60, (1_u64 << 32) / 6_000),
        (second_track, 64, 4_u64 << 32),
    ] {
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "source".into(),
                source_length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::SessionClip(clip) => clip,
            _ => panic!("expected Session MIDI clip"),
        };
        let note = match editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(duration),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        {
            Created::Note(note) => note,
            _ => panic!("expected Session MIDI note"),
        };
        if track == first_track {
            first_note = Some(note);
        }
        let device = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .expect("MIDI track has an instrument")
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: registry.default_state(id).unwrap(),
            })
            .unwrap();
    }
    let first_note = first_note.expect("first Session note was created");
    let original_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut uninterrupted = SessionPlayback::new(original_revision).unwrap();
    for playback in [&mut previous, &mut uninterrupted] {
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }

    // The short first source is finished and quiescent; the second Session
    // route still owns a held mutable occurrence at the source-edit boundary.
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    uninterrupted.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert!(
        !previous.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    editor
        .execute(Command::UpdateMidiNote {
            note: first_note,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw((1_u64 << 32) / 6_000),
            pitch: 61,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.historical_floor.is_some());
    let second_index = candidate
        .routes
        .iter()
        .position(|route| route.id == second_track)
        .unwrap();
    assert!(
        candidate.mutable_instruments[second_index]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        candidate.mutable_instruments[second_index]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        uninterrupted.mutable_instruments[1]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        "global source-floor handoff carries the unchanged route's ledger"
    );
    let mut actual = [[0.0; 2]; 32];
    let mut expected = [[0.0; 2]; 32];
    candidate.render(&mut actual).unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn mutable_arrangement_continuity_uses_track_ids_after_an_earlier_track_is_removed() {
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
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "removed mutable track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let retained_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "retained mutable track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected second MIDI track"),
    };
    for track in [first_track, retained_track] {
        let instrument = editor
            .project()
            .track(track)
            .and_then(|track| track.instrument())
            .expect("new MIDI track has an instrument")
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument,
                state: hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap(),
            })
            .unwrap();
        let clip = match editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::Clip(clip) => clip,
            _ => panic!("expected Arrangement MIDI clip"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let original_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut playback = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut oracle = SessionPlayback::new(original_revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    playback.render(&mut prefix).unwrap();
    oracle.render(&mut prefix).unwrap();

    let (_, candidate_revision) = editor
        .execute_prepared(Command::RemoveTrack(first_track), |candidate| {
            Ok::<_, hex_project::Invalid>(revision_with_instrument_registry(
                candidate,
                Arc::clone(&table),
                &registry,
            ))
        })
        .unwrap();
    let mut retained_oracle = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    retained_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut oracle_prefix = [[0.0; 2]; 32];
    retained_oracle.render(&mut oracle_prefix).unwrap();
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut playback).unwrap();
    assert_eq!(candidate.routes[0].id, retained_track);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .any(Option::is_some)
    );

    let mut actual = [[0.0; 2]; 32];
    let mut expected = [[0.0; 2]; 32];
    candidate.render(&mut actual).unwrap();
    retained_oracle.render(&mut expected).unwrap();
    assert!(
        actual[0][0] > expected[0][0],
        "removed route keeps its old-law tail"
    );
    // The removed route's declared release plus feedback tail is 16 frames;
    // after that, exact PCM must match the retained route's uninterrupted law.
    assert_eq!(&actual[16..], &expected[16..]);
}

#[test]
fn mutable_arrangement_source_change_does_not_take_the_exact_continuity_path() {
    use hex_project::{Command, Editor};

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
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let mut editor = Editor::new(source, 8).unwrap();
    let mut playback = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 8];
    playback.render(&mut prefix).unwrap();
    let frame_before_handoff = playback.performance_frame();

    let (_, candidate_revision) = editor
        .execute_prepared(
            Command::UpdateMidiNote {
                note,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(8 << 32),
                pitch: 61,
                channel: 0,
                velocity: 100,
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
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut playback),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(playback.performance_frame(), frame_before_handoff);
    assert!(
        playback
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
}

/// Field report (Pulse #3770): editing a note that is sounding under the
/// cursor refused publication with `MutableInstrumentChase`, so audio stayed on
/// the old revision. The strict planner still refuses; the live publication
/// path restarts audio at the cursor instead (playing or paused), chasing the
/// edited note, and a following edit applies too.
#[test]
fn live_handoff_restarts_an_unproven_chase_while_playing_or_paused() {
    use hex_project::{Command, Editor};

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
    let revision = |project: &hex_project::Project| {
        revision_with_instrument_registry(project, Arc::clone(&table), &registry)
    };
    for paused in [false, true] {
        let mut editor =
            Editor::new(with_test_instrument(&project(1.0, 0, 8 << 32), id, 0), 8).unwrap();
        let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
        let mut playback = SessionPlayback::new(revision(editor.project())).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 8]).unwrap();
        if paused {
            playback.apply(SessionPlaybackCommand::Pause).unwrap();
        }
        let frame = playback.performance_frame();
        editor
            .execute(Command::UpdateMidiNote {
                note,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(8 << 32),
                pitch: 61,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let edited = revision(editor.project());
        assert_eq!(
            SessionPlayback::new(Arc::clone(&edited))
                .unwrap()
                .try_inherit_state_from(&mut playback),
            Err(SessionPlaybackError::MutableInstrumentChase),
            "strict planner; paused={paused}"
        );
        let mut candidate = SessionPlayback::new(Arc::clone(&edited)).unwrap();
        candidate
            .try_inherit_state_live_from(&mut playback)
            .unwrap();
        assert_eq!(candidate.playing(), !paused);
        assert_eq!(candidate.performance_frame(), frame);

        editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
        let later_revision = revision(editor.project());
        let mut later = SessionPlayback::new(Arc::clone(&later_revision)).unwrap();
        later.try_inherit_state_live_from(&mut candidate).unwrap();
        assert_eq!(later.performance_frame(), frame);

        let mut oracle = SessionPlayback::new(later_revision).unwrap();
        oracle.play_from(frame).unwrap();
        if paused {
            later.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut actual = [[0.0; 2]; 64];
        let mut expected = actual;
        later.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "restart chases the edited note; paused={paused}"
        );
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    }
}

/// In-place bounces (Pulse #3784) bake an instrument's output into PCM, so a
/// live handoff must not carry that instrument's release tail on top of it.
/// A requested restart starts empty at the cursor; ordinary inheritance keeps
/// the removed note's tail.
#[test]
fn requested_live_restart_drops_a_removed_note_release_tail() {
    use hex_project::{Command, Editor};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 64,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = |project: &hex_project::Project| {
        revision_with_instrument_registry(project, Arc::clone(&table), &registry)
    };
    let mut editor =
        Editor::new(with_test_instrument(&project(1.0, 0, 8 << 32), id, 0), 8).unwrap();
    let note = editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
    let sounding = revision(editor.project());
    editor.execute(Command::RemoveMidiNote(note)).unwrap();
    let removed = revision(editor.project());
    let handoff = |restart: bool| {
        let mut playback = SessionPlayback::new(Arc::clone(&sounding)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 8]).unwrap();
        let frame = playback.performance_frame();
        let mut candidate = SessionPlayback::new(Arc::clone(&removed)).unwrap();
        if restart {
            candidate.request_live_restart();
        }
        assert_eq!(candidate.live_restart_requested(), restart);
        candidate
            .try_inherit_state_live_from(&mut playback)
            .unwrap();
        assert!(candidate.playing());
        assert_eq!(candidate.performance_frame(), frame);
        let mut pcm = [[0.0; 2]; 64];
        candidate.render(&mut pcm).unwrap();
        pcm
    };
    assert!(
        handoff(false).iter().flatten().any(|sample| *sample != 0.0),
        "ordinary inheritance keeps the removed note's tail"
    );
    assert_eq!(handoff(true), [[0.0; 2]; 64], "restart carries no tail");
}

#[test]
fn mutable_instrument_adds_to_hybrid_audio_before_ordered_suffix_effects() {
    use hex_lattice_contract::NativeDeviceMetadata as _;

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
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let midi_clips = match std::mem::replace(
        &mut snapshot.tracks[0].content,
        TrackContentSnapshot::Audio(Vec::new()),
    ) {
        TrackContentSnapshot::Instrument(midi_clips)
        | TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("fixture has Arrangement MIDI"),
    };
    let asset = 20;
    let audio_clip = 21;
    let effect = 22;
    snapshot.next_id = 23;
    snapshot.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "mutable-hybrid.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 16,
        playback_frames: 16,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: audio_clip,
            asset,
            start: 0,
            duration: 16,
            source_offset: 0,
            source_length: 16,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(16).unwrap(),
        }],
        midi_clips,
    };
    snapshot.tracks[0].instrument.as_mut().unwrap().state =
        hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    let gain_state = hex_devices::utility::UTILITY_DEFINITION
        .patch_parameter(
            &hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
            hex_devices::utility::parameter::GAIN,
            hex_lattice_contract::ParameterValue::Float(0.5),
        )
        .unwrap();
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: effect,
        bypassed: false,
        state: gain_state,
    }];
    snapshot.tracks[0].instrument_position = Some(0);
    routed_midi_fixture::separate_midi(&mut snapshot, 0);
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
    let asset_id = project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.3, -0.1]; 16]).unwrap(),
    )]);
    let prepared = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &project, &media, table, None, &registry,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 16];
    playback.render(&mut actual).unwrap();
    let bypassed = with_instrument_bypassed(&project, true);
    let bypassed_revision = Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            &bypassed,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut audio_only = SessionPlayback::new(bypassed_revision).unwrap();
    audio_only.apply(SessionPlaybackCommand::Play).unwrap();
    let mut audio_only_output = [[0.0; 2]; 16];
    audio_only.render(&mut audio_only_output).unwrap();

    let mut feedback = 0.0_f32;
    let expected_difference: Vec<_> = (0..16)
        .map(|_| {
            let direct = 0.2_f32;
            feedback = feedback * 0.5 + direct * 0.25;
            let post_suffix = (direct + feedback) * 0.5;
            [post_suffix, post_suffix]
        })
        .collect();
    for ((actual, audio_only), expected) in actual
        .iter()
        .zip(audio_only_output.iter())
        .zip(expected_difference.iter())
    {
        assert!((actual[0] - audio_only[0] - expected[0]).abs() <= 1.0e-6);
        assert!((actual[1] - audio_only[1] - expected[1]).abs() <= 1.0e-6);
    }
}

#[test]
fn mutable_prefix_refusal_rolls_back_audio_transport_and_occurrence_ledger() {
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
    let project = with_test_instrument(&project(0.5, 0, 8 << 32), id, 0);
    let prepared = revision_with_instrument_registry(&project, Arc::clone(&table), &registry);
    let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let track = playback.routes[0].id;
    playback.routes[0].pending = Some(PendingRoute {
        frame: 8,
        beat: BeatTime::ZERO,
        target: RouteTarget::Silence,
        current_reservation: None,
    });
    playback.apply(SessionPlaybackCommand::Play).unwrap();

    let mut refused = [[1.0; 2]; 16];
    assert_eq!(
        playback.render(&mut refused),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(refused, [[0.0; 2]; 16]);
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(playback.routes[0].pending.is_some());
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );

    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut retried = [[0.0; 2]; 16];
    playback.render(&mut retried).unwrap();

    let mut reference = SessionPlayback::new(prepared).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 16];
    reference.render(&mut expected).unwrap();
    assert_eq!(retried, expected);
    assert_eq!(playback.performance_frame(), 16);
}

#[test]
fn legacy_arrangement_renderers_refuse_enabled_mutable_instruments_atomically() {
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
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let expected_output = [[0.25, -0.25]; 8];

    let mut renderer = crate::Renderer::new(revision.arrangement());
    renderer.play();
    let mut direct_output = expected_output;
    assert!(renderer.render(&mut direct_output).is_err());
    assert_eq!(renderer.position(), 0);
    assert_eq!(direct_output, expected_output);

    let arrangement = Arc::clone(revision.arrangement());
    let mut playback = crate::playback::Playback::from_shared(arrangement);
    playback.apply(crate::playback::PlaybackCommand::Play);
    let mut shared_output = expected_output;
    assert!(playback.render(&mut shared_output).is_err());
    assert_eq!(playback.position(), 0);
    assert_eq!(shared_output, expected_output);
}

#[test]
fn mutable_short_note_releases_inside_query_and_matches_partitioned_render() {
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
    let source = with_test_instrument(&project(1.0, 0, 1 << 20), id, 0);
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = revision_with_instrument_registry(&source, table, &registry);
    let mut whole = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut partitioned = SessionPlayback::new(revision).unwrap();
    whole.apply(SessionPlaybackCommand::Play).unwrap();
    partitioned.apply(SessionPlaybackCommand::Play).unwrap();

    let mut expected = [[0.0; 2]; 32];
    let mut actual = [[0.0; 2]; 32];
    whole.render(&mut expected).unwrap();
    partitioned.render(&mut actual[..5]).unwrap();
    partitioned.render(&mut actual[5..16]).unwrap();
    partitioned.render(&mut actual[16..]).unwrap();

    assert_eq!(actual, expected);
    assert!(actual[..8].iter().any(|frame| *frame != [0.0; 2]));
    assert!(actual[24..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn mutable_coincident_release_and_attack_keep_order_across_segment_boundaries() {
    use hex_lattice::NativeInstrumentEvent;

    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let render = |segments: &[usize]| {
        let mut runtime =
            crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000)
                .unwrap();
        let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
        for (iteration, start, end, velocity) in [(0, 0, 4, 100), (1, 4, 8, 80)] {
            onsets
                .push(crate::mutable_instrument::MutableNoteOnset {
                    key: crate::mutable_instrument::MutableOccurrenceKey::new(
                        track.id(),
                        clip.id(),
                        note.id(),
                        iteration,
                        0,
                    ),
                    onset_beat: hex_project::BeatTime::from_raw(start << 32),
                    gate_end_beat: hex_project::BeatTime::from_raw(end << 32),
                    source_start: start,
                    source_gate_end: end,
                    pitch: 60,
                    channel: 0,
                    velocity,
                })
                .unwrap();
        }
        let mut audio = [[0.0; 2]; 28];
        let mut events = Vec::new();
        let mut observed = Vec::new();
        let mut start = 0;
        for &end in segments {
            runtime.begin_block();
            runtime
                .process_segment(
                    track.id(),
                    start as u64,
                    start as u64,
                    &mut audio[start..end],
                    &mut onsets,
                    &[],
                    &mut events,
                )
                .unwrap();
            observed.extend(events.iter().map(|event| match *event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    ..
                } => (start + frame_offset, true, occurrence_id),
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (start + frame_offset, false, occurrence_id),
                NativeInstrumentEvent::Expression { .. } => panic!("unexpected expression"),
            }));
            runtime.commit_block();
            start = end;
        }
        assert_eq!(start, audio.len());
        (audio, observed)
    };
    let (whole, events) = render(&[28]);
    assert_eq!(
        events,
        [(0, true, 1), (4, false, 1), (4, true, 2), (8, false, 2)]
    );
    // Independent fixture oracle: the first release remains sounding for
    // eight frames, overlapping the second note, then its feedback tail fades.
    let mut feedback = 0.0_f32;
    for (index, frame) in whole.iter().enumerate() {
        let first = if index < 12 { 0.2_f32 } else { 0.0 };
        let second = if (4..16).contains(&index) {
            0.2_f32 * 0.8
        } else {
            0.0
        };
        let direct = first + second;
        feedback = feedback * 0.5 + direct * 0.25;
        assert_eq!(*frame, [direct + feedback; 2], "frame {index}");
        if index == 22 {
            feedback = 0.0;
        }
    }
    for segments in [&[4, 8, 28][..], &[1, 3, 4, 5, 8, 9, 28][..]] {
        let (audio, actual_events) = render(segments);
        assert_eq!(actual_events, events, "segments {segments:?}");
        assert_eq!(audio, whole, "segments {segments:?}");
    }
}

#[test]
fn mutable_event_order_is_canonical_for_same_frame_notes_and_partitions() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };

    let mut all_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    // Deliberately reverse chronology and reverse the identity tie order. A
    // host/source iterator must not affect event IDs or voice-slot assignment.
    for (iteration, source_start, pitch, velocity) in [
        (4, 2, 68, 55),
        (3, 0, 72, 47),
        (2, 0, 64, 73),
        (1, 0, 59, 91),
    ] {
        all_onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    track.id(),
                    clip.id(),
                    note.id(),
                    iteration,
                    0,
                ),
                onset_beat: hex_project::BeatTime::from_raw(source_start << 32),
                gate_end_beat: hex_project::BeatTime::from_raw(8 << 32),
                source_start,
                source_gate_end: 8,
                pitch,
                channel: 0,
                velocity,
            })
            .unwrap();
    }

    let mut whole =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    whole.begin_block();
    let mut whole_output = [[0.0; 2]; 8];
    let mut whole_events = Vec::new();
    let mut whole_onsets = all_onsets.clone();
    whole
        .process_segment(
            track.id(),
            0,
            0,
            &mut whole_output,
            &mut whole_onsets,
            &[],
            &mut whole_events,
        )
        .unwrap();
    let ordered: Vec<_> = whole_events
        .iter()
        .filter_map(|event| match *event {
            hex_lattice::NativeInstrumentEvent::NoteOn {
                frame_offset,
                occurrence_id,
                pitch,
                velocity,
            } => Some((frame_offset, occurrence_id, pitch, velocity)),
            hex_lattice::NativeInstrumentEvent::NoteOff { .. }
            | hex_lattice::NativeInstrumentEvent::Expression { .. } => None,
        })
        .collect();
    assert_eq!(
        ordered,
        [
            (0, 1, 59, 91),
            (0, 2, 64, 73),
            (0, 3, 72, 47),
            (2, 4, 68, 55)
        ]
    );

    let terms = [(59_u8, 91_u8), (64_u8, 73_u8), (72_u8, 47_u8)];
    let direct = terms
        .iter()
        .map(|(pitch, velocity)| {
            0.2_f32 * (f32::from(*velocity) / 100.0) * (1.0 + (f32::from(*pitch) - 60.0) * 0.01)
        })
        .sum::<f32>();
    assert_eq!(whole_output[0], [direct + direct * 0.25; 2]);
    whole.commit_block();

    let mut partitioned =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    partitioned.begin_block();
    let mut partitioned_output = [[0.0; 2]; 8];
    let mut partitioned_events = Vec::new();
    let mut partitioned_onsets = all_onsets;
    for (start, end) in [(0_usize, 1_usize), (1, 2), (2, 5), (5, 8)] {
        partitioned
            .process_segment(
                track.id(),
                start as u64,
                start as u64,
                &mut partitioned_output[start..end],
                &mut partitioned_onsets,
                &[],
                &mut partitioned_events,
            )
            .unwrap();
    }
    assert_eq!(partitioned_output, whole_output);
    partitioned.commit_block();
}

#[test]
fn mutable_occurrence_proof_survives_gateoff_and_suppression_matches_full_signature() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let key = crate::mutable_instrument::MutableOccurrenceKey::new(
        track.id(),
        clip.id(),
        note.id(),
        3,
        2,
    );
    let onset = crate::mutable_instrument::MutableNoteOnset {
        key,
        onset_beat: BeatTime::from_raw(0),
        gate_end_beat: BeatTime::from_raw(1 << 32),
        source_start: 0,
        source_gate_end: 1,
        pitch: 60,
        channel: 4,
        velocity: 91,
    };

    let mut old =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    old.begin_block();
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    onsets.push(onset).unwrap();
    let mut events = Vec::new();
    let mut audio = [[0.0; 2]; 4];
    old.process_segment(track.id(), 0, 0, &mut audio, &mut onsets, &[], &mut events)
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [
            hex_lattice::NativeInstrumentEvent::NoteOn { .. },
            hex_lattice::NativeInstrumentEvent::NoteOff { .. }
        ]
    ));
    old.commit_block();
    let proof = old.proofs().next().expect("gateoff keeps the owned proof");
    let old_deadline = 1 + old.max_release_tail_frames();
    assert_eq!(
        proof.signature,
        crate::mutable_instrument::MutableOccurrenceSignature::new(onset)
    );
    assert_eq!(proof.onset_frame, 0);
    assert_eq!(proof.gate_end_frame, 1);
    assert_eq!(proof.quiet_deadline_frame, old_deadline);
    assert!(proof.note_off_sent);

    let suppression =
        crate::midi_template::MidiOccurrenceSuppression::mutable(proof.signature, old_deadline);
    let mut candidate =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    candidate.begin_block();
    let mut candidate_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    candidate_onsets.push(onset).unwrap();
    let mut candidate_events = Vec::new();
    let mut candidate_audio = [[0.0; 2]; 4];
    candidate
        .process_segment(
            track.id(),
            0,
            0,
            &mut candidate_audio,
            &mut candidate_onsets,
            &[suppression],
            &mut candidate_events,
        )
        .unwrap();
    assert!(candidate_events.is_empty());
    assert_eq!(candidate_audio, [[0.0; 2]; 4]);
    candidate.commit_block();

    // A changed authored payload is a distinct occurrence and must not be
    // hidden by a same-ID suppression token.
    let mut changed = onset;
    changed.pitch += 1;
    let mut changed_runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    changed_runtime.begin_block();
    let mut changed_onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    changed_onsets.push(changed).unwrap();
    let mut changed_events = Vec::new();
    let mut changed_audio = [[0.0; 2]; 4];
    changed_runtime
        .process_segment(
            track.id(),
            0,
            0,
            &mut changed_audio,
            &mut changed_onsets,
            &[suppression],
            &mut changed_events,
        )
        .unwrap();
    assert!(matches!(
        changed_events.first(),
        Some(hex_lattice::NativeInstrumentEvent::NoteOn { pitch: 61, .. })
    ));
}

#[test]
fn mutable_zero_tail_emits_exclusive_boundary_noteoff_once_across_partitions() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = ZeroTailMutableFactory;
    let mut runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    let onset = crate::mutable_instrument::MutableNoteOnset {
        key: crate::mutable_instrument::MutableOccurrenceKey::new(
            track.id(),
            clip.id(),
            note.id(),
            0,
            0,
        ),
        onset_beat: BeatTime::from_raw(0),
        gate_end_beat: BeatTime::from_raw(1 << 32),
        source_start: 0,
        source_gate_end: 4,
        pitch: 60,
        channel: 0,
        velocity: 100,
    };
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    onsets.push(onset).unwrap();
    let mut events = Vec::new();

    runtime.begin_block();
    let mut attack = [[0.0; 2]; 4];
    runtime
        .process_segment(track.id(), 0, 0, &mut attack, &mut onsets, &[], &mut events)
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [hex_lattice::NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            ..
        }]
    ));
    assert_eq!(attack, [[0.25; 2]; 4]);
    runtime.commit_block();
    let proof = runtime
        .proofs()
        .next()
        .expect("exclusive gate keeps its proof");
    assert_eq!(proof.quiet_deadline_frame, 4);
    assert!(!proof.note_off_sent);

    // The next runtime block begins exactly at the exclusive gate boundary.
    // Split it again to prove partition independence and one-shot delivery.
    runtime.begin_block();
    onsets.clear();
    let mut release_boundary = [[0.0; 2]; 1];
    runtime
        .process_segment(
            track.id(),
            4,
            4,
            &mut release_boundary,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [hex_lattice::NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            ..
        }]
    ));
    assert_eq!(release_boundary, [[0.0; 2]; 1]);

    let mut after_boundary = [[0.0; 2]; 3];
    runtime
        .process_segment(
            track.id(),
            5,
            5,
            &mut after_boundary,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(
        events.is_empty(),
        "the boundary NoteOff is sent exactly once"
    );
    assert_eq!(after_boundary, [[0.0; 2]; 3]);
    assert!(runtime.speculative_is_quiescent());
    runtime.commit_block();
    assert!(runtime.committed_is_quiescent());
    assert!(runtime.proofs().next().is_none());
}

#[test]
fn mutable_query_accepts_full_bounded_sequential_onset_and_edge_budget() {
    let source = project(1.0, 0, 8 << 32);
    let track = source.tracks().first().unwrap();
    let clip = track.midi_clips().first().unwrap();
    let note = clip.notes().first().unwrap();
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let mut runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    runtime.begin_block();
    let mut onsets = crate::mutable_instrument::MutableOnsetBuffer::default();
    for iteration in 0..128_u64 {
        let source_start = iteration * 2;
        onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    track.id(),
                    clip.id(),
                    note.id(),
                    iteration,
                    0,
                ),
                onset_beat: hex_project::BeatTime::from_raw(source_start << 32),
                gate_end_beat: hex_project::BeatTime::from_raw((source_start + 1) << 32),
                source_start,
                source_gate_end: source_start + 1,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let mut events = Vec::new();
    let mut frames = [[0.0; 2]; 256];
    runtime
        .process_segment(track.id(), 0, 0, &mut frames, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(events.len(), 256);
    let mut release = [[0.0; 2]; 16];
    onsets.clear();
    runtime
        .process_segment(
            track.id(),
            256,
            256,
            &mut release,
            &mut onsets,
            &[],
            &mut events,
        )
        .unwrap();
    assert!(runtime.speculative_is_quiescent());
    runtime.commit_block();
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn mutable_runtime_host_storage_budget_is_checked_and_reported() {
    let factory = MutableFeedbackFactory {
        level: 0.2,
        release_frames: 8,
    };
    let runtime =
        crate::mutable_instrument::MutableInstrumentTrackRuntime::new(&factory, 48_000).unwrap();
    let ledger_bytes = runtime.ledger_capacity_bytes().unwrap();
    let runtime_pair_bytes = runtime.runtime_pair_storage_bytes().unwrap();
    let graph_host_bytes =
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS).unwrap();

    assert_eq!(
        ledger_bytes,
        2 * MAX_SIMULTANEOUS_VOICES_PER_TRACK
            * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
    );
    assert_eq!(
        graph_host_bytes,
        hex_project::MAX_TRACKS
            * std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>(
            )
            + hex_project::MAX_TRACKS
                * 2
                * MAX_SIMULTANEOUS_VOICES_PER_TRACK
                * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
            + hex_lattice::MAX_STATEFUL_PROCESSORS
                * std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTail>>()
            + hex_lattice::MAX_STATEFUL_PROCESSORS
                * 2
                * MAX_SIMULTANEOUS_VOICES_PER_TRACK
                * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
            + std::mem::size_of::<crate::mutable_instrument::MutableOnsetBuffer>()
            + crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY
                * std::mem::size_of::<hex_lattice::NativeInstrumentEvent>()
            + std::mem::size_of::<
                Vec<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>,
            >()
            + std::mem::size_of::<Vec<Option<crate::mutable_instrument::MutableInstrumentTail>>>()
            + std::mem::size_of::<Vec<hex_lattice::NativeInstrumentEvent>>()
            + (hex_project::MAX_TRACKS + hex_lattice::MAX_STATEFUL_PROCESSORS)
                * crate::mutable_instrument::MutableInstrumentTrackRuntime::source_live_storage_upper_bound()
    );
    assert!(
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS + 1).is_none()
    );
    println!(
        "mutable storage bytes: track_owner={}, ledger_pair={}, onset_buffer={}, event={}, runtime_pair={}, max_graph_host={graph_host_bytes}",
        std::mem::size_of::<Option<crate::mutable_instrument::MutableInstrumentTrackRuntime>>(),
        ledger_bytes,
        std::mem::size_of::<crate::mutable_instrument::MutableOnsetBuffer>(),
        std::mem::size_of::<hex_lattice::NativeInstrumentEvent>(),
        runtime_pair_bytes,
    );
}

#[test]
fn mutable_revision_handoff_keeps_old_gate_and_suffix_until_tail_quiet() {
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
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let prepared = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let clip = prepared.session().midi_clips()[0].id;
    let start = || {
        let mut playback = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };

    let mut active = start();
    let mut reference = start();
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    active.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    let frame_before_refusal = active.performance_frame();
    assert_eq!(
        active.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(active.performance_frame(), frame_before_refusal);

    let mut candidate_snapshot = base.snapshot();
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let source_empty = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let source_empty_prepared =
        revision_with_instrument_registry(&source_empty, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(source_empty_prepared).unwrap();
    candidate.try_inherit_state_from(&mut active).unwrap();
    // Empty authoring does not replace the retained source or its live history.
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        !candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert_eq!(
        candidate.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    let mut actual_tail = [[0.0; 2]; 256];
    let mut expected_tail = [[0.0; 2]; 256];
    candidate.render(&mut actual_tail).unwrap();
    reference.render(&mut expected_tail).unwrap();
    assert_eq!(actual_tail, expected_tail);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    assert!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
}

#[test]
fn held_eq8_audition_transfers_from_mutable_release_suffix_without_transferring_filter_history() {
    use hex_lattice::{ProcessorAuditionSelection, ProcessorChainOwner};
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let id = DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut source_snapshot = with_test_instrument(&project(0.5, 0, 1 << 32), id, 0).snapshot();
    let eq8_type = DeviceTypeId::from_static("hex.eq8");
    let mut eq8_state =
        crate::default_native_device_state_for_class(crate::NativeDeviceClass::Effect, eq8_type)
            .unwrap();
    for parameter in [1, 41] {
        eq8_state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Effect,
            &eq8_state,
            ParameterId::from_static(parameter),
            ParameterValue::Bool(true),
        )
        .unwrap();
    }
    let eq8_id = source_snapshot.next_id;
    source_snapshot.next_id += 1;
    source_snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: eq8_id,
        bypassed: false,
        state: eq8_state,
    });
    source_snapshot.tracks[0].instrument_position = Some(0);
    let source_project = legacy_project_from_snapshot(source_snapshot).unwrap();
    let source_revision =
        revision_with_instrument_registry(&source_project, Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(source_revision).unwrap();
    let mut reference_previous = SessionPlayback::new(revision_with_instrument_registry(
        &source_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    reference_previous
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut playing = [[0.0; 2]; 16];
    let mut reference_playing = playing;
    previous.render(&mut playing).unwrap();
    reference_previous.render(&mut reference_playing).unwrap();
    assert!(playing.iter().any(|frame| frame[0].abs() > 1.0e-6));
    assert_eq!(
        playing, reference_playing,
        "independent source runtimes must begin with matching PCM"
    );

    let identity = crate::PostNodeIdentity::new(ProcessorChainOwner::Track(1), eq8_id, eq8_type);
    let selection = ProcessorAuditionSelection { index: 0, lane: 0 };
    assert!(previous.select_processor_audition(identity, Some(selection)));
    let mut auditioned = [[0.0; 2]; 16];
    let mut reference_auditioned = auditioned;
    previous.render(&mut auditioned).unwrap();
    reference_previous
        .render(&mut reference_auditioned)
        .unwrap();
    assert!(previous.processor_audition_is_active(identity, selection));

    // Empty the candidate's authored source while preserving the track and
    // device identities; the old live mutable note and its suffix become a
    // release owner on the candidate graph.
    let mut candidate_snapshot = source_project.snapshot();
    // A new device identity deliberately retires the old source. Clearing
    // authored clips alone keeps the existing one-source kernel in place.
    candidate_snapshot.tracks[0].instrument.as_mut().unwrap().id = candidate_snapshot.next_id;
    candidate_snapshot.next_id += 1;
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let candidate_project = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &candidate_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let mut reference_candidate = SessionPlayback::new(revision_with_instrument_registry(
        &candidate_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    reference_candidate
        .try_inherit_state_from(&mut reference_previous)
        .unwrap();
    let tail_slot = candidate
        .tail_mutable_instruments
        .iter()
        .position(Option::is_some)
        .expect("live mutable note should own a release tail");
    assert_eq!(candidate.tail_suffix_starts[tail_slot], 0);
    assert!(
        candidate.tail_groups[tail_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );

    // Place an older same-track tail with no held selector before the freshly
    // transferred tail. This catches selecting the first track match instead
    // of the exact current lease owner.
    let stale_slot = 0;
    let current_slot = if tail_slot == stale_slot {
        1
    } else {
        tail_slot
    };
    let track_id = candidate.tail_mutable_instruments[tail_slot]
        .as_ref()
        .expect("mutable release owner")
        .track_id;
    let current_chain = candidate.tail_groups[tail_slot]
        .take()
        .expect("release suffix chain");
    let current_mutable_tail = candidate.tail_mutable_instruments[tail_slot]
        .take()
        .expect("mutable release tail");
    let suffix_start = candidate.tail_suffix_starts[tail_slot];
    candidate.tail_groups[stale_slot] = Some(hex_lattice::NativeRuntimeChain::empty());
    candidate.tail_suffix_starts[stale_slot] = suffix_start;
    candidate.tail_mutable_instruments[stale_slot] = Some(MutableInstrumentTail {
        track_id,
        runtime: crate::mutable_instrument::MutableInstrumentTrackRuntime::new(
            &ZeroTailMutableFactory,
            48_000,
        )
        .unwrap(),
    });
    candidate.tail_groups[current_slot] = Some(current_chain);
    candidate.tail_suffix_starts[current_slot] = suffix_start;
    candidate.tail_mutable_instruments[current_slot] = Some(current_mutable_tail);

    assert!(candidate.transfer_processor_audition_from_release_tail(identity, selection));
    assert!(candidate.processor_audition_is_active(identity, selection));
    assert!(
        candidate.tail_groups[current_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );

    candidate.cancel_retired_processor_auditions();
    reference_candidate.cancel_retired_processor_auditions();
    assert!(candidate.processor_audition_is_active(identity, selection));
    assert!(
        !candidate.tail_groups[current_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );
    assert!(
        !candidate.tail_groups[stale_slot]
            .as_mut()
            .is_some_and(|chain| chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection
            ))
    );
    let reference_slot = reference_candidate
        .tail_groups
        .iter()
        .position(Option::is_some)
        .expect("reference mutable release suffix");
    let mut actual_tail_pcm = [[0.1, -0.2]; 256];
    let mut reference_tail_pcm = actual_tail_pcm;
    let actual_chain = candidate.tail_groups[current_slot]
        .as_mut()
        .expect("actual release suffix chain");
    actual_chain.begin_block();
    actual_chain.process_block(&mut actual_tail_pcm).unwrap();
    actual_chain.commit_block();
    let reference_chain = reference_candidate.tail_groups[reference_slot]
        .as_mut()
        .expect("reference release suffix chain");
    reference_chain.begin_block();
    reference_chain
        .process_block(&mut reference_tail_pcm)
        .unwrap();
    reference_chain.commit_block();
    assert_eq!(
        actual_tail_pcm, reference_tail_pcm,
        "hard-clearing the transferred tail selector must preserve ordinary EQ filter history PCM"
    );
}

#[test]
fn mutable_device_replacement_suppresses_exact_candidate_occurrence_across_repeated_handoff() {
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
    let source = with_test_instrument(&project(1.0, 0, 8 << 32), id, 0);
    let old_revision = revision_with_instrument_registry(&source, Arc::clone(&table), &registry);
    let track = source.tracks()[0].id();
    let clip_id = source.tracks()[0].midi_clips()[0].id();
    let note = source.tracks()[0].midi_clips()[0].notes()[0].id();
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 1];
    let mut reference_prefix = [[0.0; 2]; 1];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    let proof = previous.mutable_instruments[0]
        .as_ref()
        .and_then(|runtime| runtime.proofs().next())
        .expect("old mutable dispatch retains an occurrence proof");
    assert_eq!(proof.signature.key().track, track);
    assert_eq!(proof.signature.key().clip, clip_id);
    assert_eq!(proof.signature.key().note, note);

    let changed_source = with_test_instrument(&source, id, 1);
    let mut replacement = changed_source.snapshot();
    replacement.tracks[0].instrument.as_mut().unwrap().id = replacement.next_id;
    replacement.next_id += 1;
    let changed_source = legacy_project_from_snapshot(replacement).unwrap();
    reference
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let changed_revision =
        revision_with_instrument_registry(&changed_source, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let suppression = candidate
        .suppressions
        .iter()
        .find(|entry| entry.mutable_signature() == Some(proof.signature))
        .copied()
        .expect("exact pre-cut occurrence is suppressed for its full old/candidate tail");
    assert_eq!(suppression.track, track);
    assert!(suppression.until_frame >= proof.quiet_deadline_frame);
    let mut actual = [[0.0; 2]; 4];
    let mut expected = [[0.0; 2]; 4];
    candidate.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);

    // The carried old owner and its exact suppression move forward once;
    // subsequent publications do not create a second tail for the same voice.
    let mut next = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    next.try_inherit_state_from(&mut candidate).unwrap();
    assert_eq!(
        next.tail_mutable_instruments
            .iter()
            .flatten()
            .filter(|tail| tail.track_id == track)
            .count(),
        1
    );
    assert!(next.suppressions.iter().any(|entry| {
        entry.mutable_signature() == Some(proof.signature)
            && entry.until_frame >= suppression.until_frame
    }));
    let mut repeated = [[0.0; 2]; 4];
    let mut expected_repeated = [[0.0; 2]; 4];
    next.render(&mut repeated).unwrap();
    reference.render(&mut expected_repeated).unwrap();
    assert_eq!(repeated, expected_repeated);
}

#[test]
#[ignore = "manual release profile for repeated mutable-owner admission"]
fn profile_mutable_occurrence_suppression_handoff() {
    use std::hint::black_box;
    use std::time::Instant;

    const SAMPLES: usize = 100;
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
    let mut dense_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let first_new_id = dense_snapshot.next_id;
    dense_snapshot.next_id += 63;
    let clips = match &mut dense_snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips,
        TrackContentSnapshot::Audio(_) => panic!("profile fixture has MIDI content"),
    };
    let notes = &mut clips[0].notes;
    for index in 1..64_u64 {
        notes.push(MidiNoteSnapshot {
            active: true,
            id: first_new_id + index - 1,
            start_raw: 0,
            duration_raw: 8 << 32,
            pitch: u8::try_from(40 + index).unwrap(),
            channel: 0,
            velocity: 100,
        });
    }
    let dense_source = legacy_project_from_snapshot(dense_snapshot).unwrap();
    let source = with_test_instrument(&dense_source, id, 0);
    let mut playback = SessionPlayback::new(revision_with_instrument_registry(
        &source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 1];
    playback.render(&mut prefix).unwrap();
    assert_eq!(
        playback.mutable_instruments[0]
            .as_ref()
            .map_or(0, |runtime| runtime.proofs().count()),
        64
    );
    let mut timings = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        // Candidate preparation and destruction of the previous publication
        // stay outside the measured handoff call.
        let state = u8::try_from((sample + 1) % 2).unwrap();
        let candidate_source = with_test_instrument(&source, id, state);
        let revision =
            revision_with_instrument_registry(&candidate_source, Arc::clone(&table), &registry);
        let mut candidate = SessionPlayback::new(revision).unwrap();
        let start = Instant::now();
        candidate
            .try_inherit_state_from(black_box(&mut playback))
            .unwrap();
        timings.push(start.elapsed().as_nanos());
        black_box(candidate.suppressions.len());
        playback = candidate;
    }
    timings.sort_unstable();
    let p50 = timings[SAMPLES / 2];
    let p95 = timings[(SAMPLES * 95).div_ceil(100) - 1];
    let p99 = timings[(SAMPLES * 99).div_ceil(100) - 1];
    println!(
        "mutable occurrence handoff: samples={SAMPLES} dense_active_occurrences=64 frame={} proofs={} suppressions={} ledger_pair_bytes={} max_graph_host_bytes={}",
        playback.performance_frame(),
        playback.mutable_instruments[0]
            .as_ref()
            .map_or(0, |runtime| runtime.proofs().count()),
        playback.suppressions.len(),
        2 * crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS
            * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>(),
        crate::mutable_instrument::host_storage_upper_bound(hex_project::MAX_TRACKS).unwrap(),
    );
    println!(
        "handoff_p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        p50 as f64 / 1_000.0,
        p95 as f64 / 1_000.0,
        p99 as f64 / 1_000.0,
        timings[SAMPLES - 1] as f64 / 1_000.0,
    );
}

#[test]
fn mutable_to_analytic_handoff_refuses_active_chase_then_preserves_old_gate_and_suffix() {
    // Keep the seven independent oracle/candidate owners out of this test's
    // debug frame. Production handoff still runs on the default test stack.
    #[inline(never)]
    fn boxed_playback(revision: Arc<PreparedRevision>) -> Box<SessionPlayback> {
        Box::new(SessionPlayback::new(revision).unwrap())
    }

    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let mutable_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let analytic_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.analytic");
    let mutable = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: mutable_id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let analytic = ContinuityInstrumentDefinition {
        id: analytic_id,
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 2] = [&mutable, &analytic];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());

    let mut source = with_test_instrument(&project(0.5, 0, 1 << 24), mutable_id, 0).snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_remove = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    let first_id = source.next_id;
    source.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: first_id + 1,
            bypassed: false,
            state: dc_remove,
        },
    ];
    source.next_id += 2;
    let midi_clips = match std::mem::replace(
        &mut source.tracks[0].content,
        TrackContentSnapshot::Audio(Vec::new()),
    ) {
        TrackContentSnapshot::Instrument(midi_clips)
        | TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("fixture has Arrangement MIDI"),
    };
    let asset = source.next_id;
    let audio_clip = asset + 1;
    source.next_id += 2;
    source.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "mutable-tail-input.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 64,
        playback_frames: 64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    source.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
            active: true,
            name: "clip".into(),
            flavor: hex_project::ClipFlavor::default(),
            edit: None,
            gain: hex_project::ClipGain::UNITY,
            id: audio_clip,
            asset,
            start: 0,
            duration: 64,
            source_offset: 0,
            source_length: 64,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(64).unwrap(),
        }],
        midi_clips,
    };
    source.tracks[0].instrument_position = Some(1);
    routed_midi_fixture::separate_midi(&mut source, 0);
    let old_project = Project::from_legacy_snapshot(source).unwrap();
    let asset_id = old_project.assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset_id,
        crate::Audio::new(48_000, vec![[0.1, 0.03]; 64]).unwrap(),
    )]);
    let prepare = |project: &Project, table: Arc<hex_devices::SineTable>| {
        Arc::new(
            PreparedRevision::prepare_with_instrument_registry(
                project, &media, table, None, &registry,
            )
            .unwrap(),
        )
    };
    let old_revision = prepare(&old_project, Arc::clone(&table));
    let mut previous = boxed_playback(Arc::clone(&old_revision));
    let mut reference = boxed_playback(old_revision);
    for playback in [&mut previous, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

    // The unchanged, enabled analytic candidate would sound the same
    // pre-cut occurrence alongside the retained mutable owner. Refuse it
    // atomically until mutable occurrence suppression is available.
    let changed_source = with_test_instrument(&old_project, analytic_id, 0);
    let changed_revision = prepare(&changed_source, Arc::clone(&table));
    let mut refused = boxed_playback(changed_revision);
    let previous_frame = previous.performance_frame();
    assert_eq!(
        refused.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert_eq!(previous.performance_frame(), previous_frame);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );

    // A source-empty enabled analytic replacement is safe. The old held
    // occurrence keeps its original gate and captured DC-removal suffix.
    let mut tail_snapshot = old_project.snapshot();
    if let TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips,
    } = &mut tail_snapshot.tracks[0].content
    {
        audio_clips.clear();
        midi_clips.clear();
    }
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut tail_snapshot.tracks[1].content {
        midi_clips.clear();
    }
    tail_snapshot.session.cells.clear();
    let tail_project = Project::from_legacy_snapshot(tail_snapshot).unwrap();
    let mut tail_candidate = boxed_playback(prepare(&tail_project, Arc::clone(&table)));
    tail_candidate
        .try_inherit_state_from(&mut reference)
        .unwrap();

    let mut candidate_snapshot = old_project.snapshot();
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut candidate_snapshot.tracks[0].content
    {
        midi_clips.clear();
    }
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut candidate_snapshot.tracks[1].content
    {
        midi_clips.clear();
    }
    candidate_snapshot.session.cells.clear();
    let candidate_source = Project::from_legacy_snapshot(candidate_snapshot).unwrap();
    let empty_analytic = with_test_instrument(&candidate_source, analytic_id, 0);
    let mut replaced = empty_analytic.snapshot();
    replaced.tracks[0].instrument.as_mut().unwrap().id = replaced.next_id;
    replaced.next_id += 1;
    let empty_analytic = legacy_project_from_snapshot(replaced).unwrap();
    tail_candidate
        .apply(SessionPlaybackCommand::StopTrack {
            track: old_project.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let empty_revision = prepare(&empty_analytic, Arc::clone(&table));
    let mut candidate = boxed_playback(empty_revision);
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let mut actual = [[0.0; 2]; 16];
    let mut old_tail_pcm = [[0.0; 2]; 16];
    let mut new_audio_pcm = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    tail_candidate.render(&mut old_tail_pcm).unwrap();
    let mut fresh_audio = boxed_playback(prepare(&candidate_source, Arc::clone(&table)));
    fresh_audio.apply(SessionPlaybackCommand::Seek(16)).unwrap();
    fresh_audio.apply(SessionPlaybackCommand::Play).unwrap();
    fresh_audio.render(&mut new_audio_pcm).unwrap();
    for ((actual, old_tail), new_audio) in actual
        .iter()
        .zip(old_tail_pcm.iter())
        .zip(new_audio_pcm.iter())
    {
        assert!((actual[0] - old_tail[0] - new_audio[0]).abs() <= 1.0e-6);
        assert!((actual[1] - old_tail[1] - new_audio[1]).abs() <= 1.0e-6);
    }
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));

    // A retained old mutable owner also fences a later candidate that would
    // reintroduce the still-sounding pre-cut occurrence.
    let later_active = with_test_instrument(&old_project, analytic_id, 0);
    let later_revision = prepare(&later_active, Arc::clone(&table));
    let mut retained_tail_chase = boxed_playback(later_revision);
    let tail_frame = candidate.performance_frame();
    assert_eq!(
        retained_tail_chase.try_inherit_state_from(&mut candidate),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(candidate.performance_frame(), tail_frame);
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| !tail.runtime.committed_is_quiescent())
    );
}

#[test]
fn mutable_handoff_keeps_old_suffix_and_refusal_is_retryable_after_tail_processing() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let mut old_snapshot = base.snapshot();
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let dc_remove = UTILITY_DEFINITION
        .patch_parameter(&neutral, parameter::DC_REMOVE, ParameterValue::Bool(true))
        .unwrap();
    let first_id = old_snapshot.next_id;
    old_snapshot.tracks[0].devices = vec![
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: first_id + 1,
            bypassed: false,
            state: dc_remove,
        },
    ];
    old_snapshot.next_id += 2;
    old_snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let start = || {
        let mut playback = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback
    };
    let mut previous = start();
    let mut reference = start();
    let mut prefix = [[0.0; 2]; 16];
    let mut reference_prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    // Keep the instrument enabled but remove candidate source occurrences.
    // Existing mutable voices must retain their original gate deadline.
    let mut candidate_snapshot = old_project.snapshot();
    // A new device identity deliberately retires the old source. Clearing
    // authored clips alone keeps the existing one-source kernel in place.
    candidate_snapshot.tracks[0].instrument.as_mut().unwrap().id = candidate_snapshot.next_id;
    candidate_snapshot.next_id += 1;
    candidate_snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    candidate_snapshot.session.cells.clear();
    let candidate_project = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let candidate_project = with_utility_filter_state_at(&candidate_project, 1, false, true);
    let candidate_revision =
        revision_with_instrument_registry(&candidate_project, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();

    // A full preallocated tail bank refuses before either graph is mutated.
    for slot in &mut candidate.tail_groups {
        *slot = Some(hex_lattice::NativeRuntimeChain::empty());
    }
    let previous_frame = previous.performance_frame;
    let previous_routes = previous.routes.clone();
    let previous_proofs: Vec<_> = previous.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .map(|proof| {
            (
                proof.signature,
                proof.id,
                proof.onset_frame,
                proof.gate_end_frame,
                proof.quiet_deadline_frame,
                proof.note_off_sent,
            )
        })
        .collect();
    let previous_suppressions = previous.suppressions.clone();
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(previous.performance_frame, previous_frame);
    assert_eq!(previous.routes, previous_routes);
    assert_eq!(previous.suppressions, previous_suppressions);
    assert_eq!(
        previous.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .proofs()
            .map(|proof| {
                (
                    proof.signature,
                    proof.id,
                    proof.onset_frame,
                    proof.gate_end_frame,
                    proof.quiet_deadline_frame,
                    proof.note_off_sent,
                )
            })
            .collect::<Vec<_>>(),
        previous_proofs
    );
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    for slot in &mut candidate.tail_groups {
        *slot = None;
    }
    // Replacing the source releases the old clip gate. The independent
    // stopped-track oracle retains the exact same original suffix history.
    reference
        .apply(SessionPlaybackCommand::StopTrack {
            track: old_project.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(previous.mutable_instruments[0].is_none());
    let tail_group = candidate
        .tail_mutable_instruments
        .iter()
        .position(Option::is_some)
        .unwrap();
    assert_eq!(candidate.tail_suffix_starts[tail_group], 1);
    assert_eq!(candidate.tail_groups[tail_group].as_ref().unwrap().len(), 2);

    // Fail after the mutable tail has processed but before the captured suffix
    // completes. The block transaction must roll its runtime back for retry.
    candidate.tail_suffix_starts[tail_group] = usize::MAX;
    let frame_before_failure = candidate.performance_frame;
    let mut failed = [[1.0; 2]; 16];
    assert_eq!(
        candidate.render(&mut failed),
        Err(SessionPlaybackError::DeviceRuntime)
    );
    assert_eq!(failed, [[0.0; 2]; 16]);
    assert_eq!(candidate.performance_frame, frame_before_failure);
    candidate.tail_suffix_starts[tail_group] = 1;

    let mut actual = [[0.0; 2]; 16];
    let mut expected = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));
    assert!(
        candidate.tail_mutable_instruments[tail_group]
            .as_ref()
            .is_some_and(|tail| !tail.runtime.committed_is_quiescent())
    );

    // A second publication carries the same still-live old owner without
    // recapturing or resetting its held occurrence.
    let mut next = SessionPlayback::new(Arc::clone(&candidate.revision)).unwrap();
    next.try_inherit_state_from(&mut candidate).unwrap();
    let mut next_pcm = [[0.0; 2]; 16];
    let mut reference_pcm = [[0.0; 2]; 16];
    next.render(&mut next_pcm).unwrap();
    reference.render(&mut reference_pcm).unwrap();
    assert_eq!(next_pcm, reference_pcm);
    assert!(
        next.tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| { !tail.runtime.committed_is_quiescent() })
    );
}

#[test]
fn mutable_handoff_to_disabled_instrument_cuts_held_gate_but_keeps_release_state() {
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

    // A held occurrence is cut at the handoff frame. Compare its first release
    // block to an independent instance of the original feedback law.
    let base = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 16];
    previous.render(&mut prefix).unwrap();

    let disabled = with_instrument_bypassed(&base, true);
    let disabled_revision =
        revision_with_instrument_registry(&disabled, Arc::clone(&table), &registry);
    let mut candidate = SessionPlayback::new(disabled_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let tail_index = candidate
        .tail_mutable_instruments
        .iter()
        .position(Option::is_some)
        .unwrap();

    let mut oracle = MutableFeedbackRuntime {
        key: 0.2_f32.to_bits(),
        level: 0.2,
        release_frames: 8,
        voices: [MutableFeedbackVoice::default(); 64],
        feedback: 0.0,
        tail_left: 0,
        storage: Box::default(),
    };
    let note_on = [hex_lattice::NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 100,
    }];
    let mut oracle_prefix = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(
        &mut oracle,
        &note_on,
        &mut oracle_prefix,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let note_off = [hex_lattice::NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 1,
    }];
    let mut expected = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(
        &mut oracle,
        &note_off,
        &mut expected,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    for frame in &mut expected {
        frame[0] *= 0.5;
        frame[1] *= 0.5;
    }
    let mut actual = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(
        candidate.tail_mutable_instruments[tail_index]
            .as_ref()
            .is_some_and(|tail| tail.runtime.committed_is_quiescent())
    );

    // An occurrence already in its release phase has no pending ledger gate;
    // disabling the candidate must preserve that old in-flight state exactly.
    let short = with_test_instrument(&project(0.5, 0, 1 << 24), id, 0);
    let short_revision = revision_with_instrument_registry(&short, Arc::clone(&table), &registry);
    let short_clip = short_revision.session().midi_clips()[0].id;
    let mut old = SessionPlayback::new(Arc::clone(&short_revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&short_revision)).unwrap();
    for playback in [&mut old, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: short_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut old_prefix = [[0.0; 2]; 96];
    let mut reference_prefix = [[0.0; 2]; 96];
    old.render(&mut old_prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(old_prefix, reference_prefix);
    assert!(
        old.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let disabled_short = with_instrument_bypassed(&short, true);
    let disabled_short_revision =
        revision_with_instrument_registry(&disabled_short, Arc::clone(&table), &registry);
    let mut next = SessionPlayback::new(disabled_short_revision).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual_release = [[0.0; 2]; 16];
    let mut expected_release = [[0.0; 2]; 16];
    next.render(&mut actual_release).unwrap();
    reference.render(&mut expected_release).unwrap();
    assert_eq!(actual_release, expected_release);
}

#[test]
fn activation_mute_cuts_held_mutable_gate_with_instrument_still_enabled() {
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
    for session in [false, true] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
        let clip = if session {
            old_revision.session().midi_clips()[0].id
        } else {
            base.tracks()[0].midi_clips()[0].id()
        };
        let mut old = SessionPlayback::new(old_revision).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetClipActive {
                clip,
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        // Clip activation changes the authored gate, not the source DSP owner.
        assert!(
            candidate
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        let source = candidate.mutable_instruments[0].as_ref().unwrap();
        assert!(source.proofs().all(|proof| proof.gate_end_frame == 16));
        let mut actual = [[0.0; 2]; 16];
        candidate.render(&mut actual).unwrap();
        assert!(actual[..8].iter().any(|frame| frame[0] != 0.0));
        assert!(
            candidate.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .committed_is_quiescent()
        );
        assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    }
}

#[test]
fn note_activation_cuts_the_held_mutable_gate() {
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
    for session in [false, true] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let old_revision = revision_with_instrument_registry(&base, Arc::clone(&table), &registry);
        let clip = if session {
            old_revision.session().midi_clips()[0].id
        } else {
            base.tracks()[0].midi_clips()[0].id()
        };
        let note = base.midi_source(clip).unwrap().notes()[0].id();
        let mut old = SessionPlayback::new(old_revision).unwrap();
        if session {
            old.apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        }
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 16]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetMidiNotesActive {
                clip,
                notes: vec![note],
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let source = candidate.mutable_instruments[0].as_ref().unwrap();
        assert!(source.proofs().all(|proof| proof.gate_end_frame == 16));
        let mut released = [[0.0; 2]; 16];
        candidate.render(&mut released).unwrap();
        assert!(
            candidate.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .committed_is_quiescent()
        );
        assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    }
}

#[test]
fn disabling_one_note_keeps_another_held_note_sounding() {
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
    let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
    let mut editor = Editor::new(base, 8).unwrap();
    let clip = editor.project().tracks()[0].midi_clips()[0].id();
    let muted_note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let Created::Note(other_note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1 << 28),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note")
    };
    let old_revision =
        revision_with_instrument_registry(editor.project(), Arc::clone(&table), &registry);
    let mut old = SessionPlayback::new(old_revision).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![muted_note],
            active: false,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    let proofs = candidate.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .collect::<Vec<_>>();
    assert_eq!(
        proofs
            .iter()
            .find(|proof| proof.signature.key().note == muted_note)
            .unwrap()
            .gate_end_frame,
        16
    );
    assert!(
        proofs
            .iter()
            .find(|proof| proof.signature.key().note == other_note)
            .unwrap()
            .gate_end_frame
            > 16
    );
    let mut output = [[0.0; 2]; 16];
    candidate.render(&mut output).unwrap();
    assert!(output[8..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn activation_time_span_shortens_mutable_prefix_and_retriggers_right_window() {
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
    for publish in [16usize, 48] {
        let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
        let track = base.tracks()[0].id();
        let mut old = SessionPlayback::new(revision_with_instrument_registry(
            &base,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut vec![[0.0; 2]; publish]).unwrap();
        let mut editor = Editor::new(base, 8).unwrap();
        editor
            .execute(Command::SetTimeRangeActive {
                range: hex_project::TimeRange::new(vec![track], 32, 64).unwrap(),
                active: false,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            editor.project(),
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut old).unwrap();
        let expected_gate = 32u64.max(publish as u64);
        assert!(
            candidate
                .tail_mutable_instruments
                .iter()
                .all(Option::is_none)
        );
        assert!(
            candidate.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .any(|proof| proof.gate_end_frame == expected_gate)
        );
        // A repeated publication retains the admitted shortened gate, including
        // the prospective proof carried under the new canonical signature.
        let mut next = SessionPlayback::new(Arc::clone(&candidate.revision)).unwrap();
        next.try_inherit_state_from(&mut candidate).unwrap();
        let mut output = vec![[0.0; 2]; 80 - publish];
        next.render(&mut output).unwrap();
        assert!(output[64 - publish..].iter().any(|frame| frame[0] != 0.0));
        assert!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .any(|proof| proof.onset_frame == 64)
        );
        assert!(next.tail_mutable_instruments.iter().all(Option::is_none));
        assert!(
            next.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .proofs()
                .filter(|proof| proof.onset_frame < 64)
                .all(|proof| proof.note_off_sent)
        );
    }
}

#[test]
fn activation_of_other_clip_keeps_existing_mutable_gate_deadline() {
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
    let base = with_test_instrument(&project(0.5, 0, 1 << 28), id, 0);
    let track = base.tracks()[0].id();
    let first = base.tracks()[0].midi_clips()[0].id();
    let mut editor = Editor::new(base, 8).unwrap();
    let Created::Clip(other) = editor.execute(Command::DuplicateMidiClip(first)).unwrap() else {
        panic!("clip")
    };
    let mut old = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    let deadline = old.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .next()
        .unwrap()
        .gate_end_frame;
    editor
        .execute(Command::SetClipActive {
            clip: other,
            active: false,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .all(Option::is_none)
    );
    let proof = candidate.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .proofs()
        .find(|proof| proof.signature.key().clip == first)
        .unwrap();
    assert_eq!(proof.gate_end_frame, deadline);
    assert!(candidate.revision.arrangement().tracks()[0].instrument_slot_enabled());
    assert!(candidate.revision.arrangement().tracks()[0].canonical_track_id() == Some(track));
}

#[test]
fn mutable_source_route_switch_releases_old_gate_without_resetting_kernel() {
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
    let source = with_test_instrument(&project(1.0, 0, 1 << 20), id, 0);
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut onset = [[0.0; 2]; 1];
    playback.render(&mut onset).unwrap();
    let cut = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        }),
        Ok(())
    );
    assert_eq!(playback.performance_frame(), cut);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    let mut oracle = MutableFeedbackRuntime {
        key: 0.2_f32.to_bits(),
        level: 0.2,
        release_frames: 8,
        voices: [MutableFeedbackVoice::default(); 64],
        feedback: 0.0,
        tail_left: 0,
        storage: Box::default(),
    };
    use hex_lattice::MutableInstrumentRuntime;
    use hex_lattice::NativeInstrumentEvent::{NoteOff, NoteOn};
    let mut oracle_prefix = [[0.0; 2]; 1];
    oracle.process_block(
        &[NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        }],
        &mut oracle_prefix,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(onset, oracle_prefix);

    let mut release = [[0.0; 2]; 32];
    playback.render(&mut release).unwrap();
    let mut expected = [[0.0; 2]; 32];
    oracle.process_block(
        &[
            NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            },
            NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            },
            NoteOff {
                frame_offset: 6,
                occurrence_id: 2,
            },
        ],
        &mut expected,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(
        release, expected,
        "one uninterrupted DSP handles both route generations"
    );
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("quiet mutable owner may switch routes without losing note events");
    assert!(matches!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip: active }) if active == clip
    ));
    let mut new_source = [[0.0; 2]; 1];
    playback.render(&mut new_source).unwrap();
    assert_eq!(new_source[0], [0.25, 0.25]);
}

#[test]
fn mutable_source_back_to_arrangement_does_not_chase_pre_cut_notes() {
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
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let arrangement_clips = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        TrackContentSnapshot::Audio(_) => panic!("Arrangement MIDI fixture expected"),
    };
    arrangement_clips[0].notes[0].duration_raw = 8 << 32;
    let SessionCellContentSnapshot::Midi(session_clip) = &mut snapshot.session.cells[0].content
    else {
        panic!("Session MIDI fixture expected");
    };
    session_clip.source.notes[0].duration_raw = 1 << 32;
    let source = legacy_project_from_snapshot(snapshot).unwrap();
    let source = with_test_instrument(&source, id, 0);
    let revision = revision_with_instrument_registry(
        &source,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let clip = revision.session().midi_clips()[0].id;
    let track = revision.session().midi_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut session_audio = vec![[0.0; 2]; 24_064];
    playback.render(&mut session_audio).unwrap();
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .is_some_and(MutableInstrumentTrackRuntime::committed_is_quiescent)
    );
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let cut = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
        Ok(())
    );
    assert_eq!(playback.performance_frame(), cut);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    let mut no_chase = [[1.0; 2]; 64];
    playback.render(&mut no_chase).unwrap();
    assert_eq!(no_chase, [[0.0; 2]; 64]);
    assert!(
        playback.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );

    playback
        .apply(SessionPlaybackCommand::Seek(16 * 24_000 + 1))
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .expect("retry succeeds once the Arrangement destination is clear");
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn mutable_prepared_playback_admits_bounded_arrangement_transport_loops() {
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

    let mut arrangement_loop = project(0.5, 0, 1 << 24).snapshot();
    arrangement_loop.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .state = hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    arrangement_loop.loop_region = Some(LoopRegion::new(0, 48_000, true).unwrap());
    let arrangement_loop = legacy_project_from_snapshot(arrangement_loop).unwrap();
    let prepared =
        revision_with_instrument_registry(&arrangement_loop, Arc::clone(&table), &registry);
    assert!(SessionPlayback::new(prepared).is_ok());

    let mut session_loop = session_only_project(&project(0.5, 0, 1 << 24)).snapshot();
    session_loop.tracks[0].instrument.as_mut().unwrap().state =
        hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    let SessionCellContentSnapshot::Midi(clip) = &mut session_loop.session.cells[0].content else {
        panic!("fixture has a Session MIDI source")
    };
    clip.source.looping = true;
    let session_loop = legacy_project_from_snapshot(session_loop).unwrap();
    let prepared = revision_with_instrument_registry(&session_loop, table, &registry);
    assert!(SessionPlayback::new(prepared).is_ok());
}

#[test]
fn changing_session_instrument_identity_carries_old_held_voice_once() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    let replaced = with_replaced_instrument_id(&original);
    let mut candidate = SessionPlayback::new(revision(&replaced, table)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.releases.iter().any(|entry| {
        entry.track_id == candidate.routes[0].id
            && entry.continuation.is_some()
            && entry.timeline == ReleaseTimeline::Session
    }));

    let mut expected = [[0.0; 2]; 128];
    let mut actual = [[0.0; 2]; 128];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn session_bypass_releases_old_held_voice_and_reenable_does_not_chase_it() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    assert!(prefix.iter().any(|frame| frame[0] != 0.0));

    let bypassed = with_instrument_bypassed(&original, true);
    let mut muted_revision = SessionPlayback::new(revision(&bypassed, Arc::clone(&table))).unwrap();
    muted_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    assert!(muted_revision.releases.iter().any(|entry| {
        entry.track_id == muted_revision.routes[0].id
            && entry.continuation.is_none()
            && entry.voice.gate_end_frame() == muted_revision.performance_frame
    }));
    assert!(muted_revision.suppressions.is_empty());

    let mut tail = vec![[0.0; 2]; 2_500];
    muted_revision.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
    assert!(tail[2_400..].iter().all(|frame| *frame == [0.0; 2]));

    let enabled = with_instrument_bypassed(&bypassed, false);
    let mut reenabled = SessionPlayback::new(revision(&enabled, Arc::clone(&table))).unwrap();
    reenabled
        .try_inherit_state_from(&mut muted_revision)
        .unwrap();
    let mut after_reenable = [[0.0; 2]; 64];
    reenabled.render(&mut after_reenable).unwrap();
    assert!(after_reenable.iter().all(|frame| *frame == [0.0; 2]));
    assert!(reenabled.route_historical_floors[0].is_some());

    let removed = without_instrument_slot(&enabled);
    let mut removed_playback =
        SessionPlayback::new(revision(&removed, Arc::clone(&table))).unwrap();
    removed_playback
        .try_inherit_state_from(&mut reenabled)
        .unwrap();
    assert!(removed_playback.route_historical_floors[0].is_some());
    let readded = with_inserted_instrument_slot(
        &removed,
        original.snapshot().tracks[0]
            .instrument
            .as_ref()
            .unwrap()
            .state
            .clone(),
    );
    let mut readded_playback = SessionPlayback::new(revision(&readded, table)).unwrap();
    readded_playback
        .try_inherit_state_from(&mut removed_playback)
        .unwrap();
    assert!(readded_playback.route_historical_floors[0].is_some());
    let mut after_readd = [[0.0; 2]; 64];
    readded_playback.render(&mut after_readd).unwrap();
    assert!(after_readd.iter().all(|frame| *frame == [0.0; 2]));
    readded_playback
        .apply(SessionPlaybackCommand::Seek(0))
        .unwrap();
    assert_eq!(readded_playback.route_historical_floors[0], None);
    let mut after_seek = [[0.0; 2]; 64];
    readded_playback.render(&mut after_seek).unwrap();
    assert!(after_seek.iter().any(|frame| frame[0] != 0.0));
    readded_playback
        .apply(SessionPlaybackCommand::Stop)
        .unwrap();
    assert_eq!(readded_playback.route_historical_floors[0], None);
}

#[test]
fn reenable_floor_excludes_old_onsets_but_keeps_note_at_exact_cut() {
    let table = Arc::new(hex_devices::SineTable::new());
    let with_notes =
        with_session_note_at(&session_only_project(&project(0.5, 0, 8 << 32)), 1 << 32);
    let disabled = with_instrument_bypassed(&with_notes, true);
    let mut previous = SessionPlayback::new(revision(&disabled, Arc::clone(&table))).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut silent_prefix = vec![[0.0; 2]; 24_000];
    previous.render(&mut silent_prefix).unwrap();
    assert!(silent_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let enabled = with_instrument_bypassed(&with_notes, false);
    let mut candidate = SessionPlayback::new(revision(&enabled, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.route_historical_floors[0], Some(BeatTime::ONE));

    let mut oracle_snapshot = enabled.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut oracle_snapshot.session.cells[0].content {
        clip.source.notes.retain(|note| note.start_raw == 1 << 32);
    }
    let oracle_project = legacy_project_from_snapshot(oracle_snapshot).unwrap();
    let mut oracle = SessionPlayback::new(revision(&oracle_project, table)).unwrap();
    let oracle_clip = oracle.revision.session().midi_clips()[0].id;
    oracle
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: oracle_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut oracle_prefix = vec![[0.0; 2]; 24_000];
    oracle.render(&mut oracle_prefix).unwrap();
    assert!(oracle_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn reenabled_loop_source_skips_old_occurrences_and_plays_future_iteration() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = session_only_project(&project(0.5, 0, 8 << 32));
    let mut snapshot = base.snapshot();
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("fixture has a Session MIDI clip")
    };
    clip.source.source_length_raw = 4 << 32;
    clip.source.loop_start_raw = 0;
    clip.source.loop_end_raw = 1 << 32;
    clip.source.looping = true;
    clip.source.notes = vec![MidiNoteSnapshot {
        active: true,
        id: clip.source.notes[0].id,
        start_raw: 1 << 30,
        duration_raw: 3 << 32,
        pitch: 60,
        channel: 0,
        velocity: 100,
    }];
    let looping = legacy_project_from_snapshot(snapshot).unwrap();
    let disabled = with_instrument_bypassed(&looping, true);
    let mut previous = SessionPlayback::new(revision(&disabled, Arc::clone(&table))).unwrap();
    let clip_id = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: clip_id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut disabled_prefix = vec![[0.0; 2]; 36_000];
    previous.render(&mut disabled_prefix).unwrap();
    assert!(disabled_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let enabled = with_instrument_bypassed(&looping, false);
    let mut candidate = SessionPlayback::new(revision(&enabled, table)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(
        candidate.route_historical_floors[0],
        Some(BeatTime::from_ratio(3, 2).unwrap())
    );

    let mut before_next_iteration = vec![[0.0; 2]; 18_000];
    candidate.render(&mut before_next_iteration).unwrap();
    assert!(before_next_iteration.iter().all(|frame| *frame == [0.0; 2]));
    let mut next_iteration = [[0.0; 2]; 128];
    candidate.render(&mut next_iteration).unwrap();
    assert!(next_iteration.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn removing_session_instrument_releases_old_voice_without_candidate_suppression() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let removed = without_instrument_slot(&original);
    let mut candidate = SessionPlayback::new(revision(&removed, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.suppressions.is_empty());
    assert!(
        candidate.releases.iter().any(|entry| {
            entry.track_id == candidate.routes[0].id && entry.continuation.is_none()
        })
    );
    let mut tail = vec![[0.0; 2]; 2_500];
    candidate.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
    assert!(tail[2_400..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn session_instrument_change_at_note_onset_leaves_boundary_note_to_candidate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project_at_tempo(0.5, 1 << 32, 8 << 32, Tempo::DEFAULT));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut before = vec![[0.0; 2]; 24_000];
    let mut reference_before = vec![[0.0; 2]; 24_000];
    previous.render(&mut before).unwrap();
    reference.render(&mut reference_before).unwrap();
    assert_eq!(before, reference_before);
    assert!(before.iter().all(|frame| *frame == [0.0; 2]));

    let replaced = with_replaced_instrument_id(&original);
    let mut candidate = SessionPlayback::new(revision(&replaced, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.releases.is_empty());
    let mut expected = [[0.0; 2]; 2];
    let mut actual = [[0.0; 2]; 2];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(actual[1], [0.0; 2]);
}

#[test]
fn arrangement_instrument_edits_keep_old_occurrence_once_and_use_new_law_afterward() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));

    for (candidate_type, candidate_state, expected_onset, replace_id) in [
        (left.id, 0, 0.4_f32, false),
        (left.id, 1, 0.6_f32, false),
        (right.id, 0, 0.9_f32, false),
        (left.id, 0, 0.4_f32, true),
    ] {
        let old_project = with_test_instrument(&base, left.id, 0);
        let old_revision =
            revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
        let mut previous = Playback::from_prepared(Arc::clone(&old_revision));
        let mut reference = Playback::from_prepared(old_revision);
        assert_eq!(
            previous.session_capability(),
            crate::playback::SessionCapability::Available
        );
        previous.apply(PlaybackCommand::Play);
        reference.apply(PlaybackCommand::Play);
        let mut prefix = [[0.0; 2]; 64];
        let mut reference_prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        reference.render(&mut reference_prefix).unwrap();
        assert_eq!(prefix, [[0.2; 2]; 64]);
        assert_eq!(prefix, reference_prefix);

        let changed = with_test_instrument(&base, candidate_type, candidate_state);
        let changed = if replace_id {
            with_replaced_instrument_id(&changed)
        } else {
            changed
        };
        let candidate_revision =
            revision_with_instrument_registry(&changed, Arc::clone(&table), &registry);
        let mut candidate = Playback::from_prepared(candidate_revision);
        candidate.try_inherit_runtime_from(&mut previous).unwrap();

        let mut held = [[0.0; 2]; 64];
        candidate.render(&mut held).unwrap();
        assert_eq!(held, [[0.2; 2]; 64]);
        let mut until_second = vec![[0.0; 2]; 24_000 - 128];
        candidate.render(&mut until_second).unwrap();
        let mut at_second = [[0.0; 2]; 1];
        candidate.render(&mut at_second).unwrap();
        assert_eq!(at_second[0], [expected_onset, expected_onset]);
    }
}

#[test]
fn instrument_edit_suppressions_end_at_the_loop_wrap_across_block_sizes() {
    use hex_lattice::NativeInstrumentDefinition;

    // Pulse #4135: the held note (beats 0..8) outlasts the loop over frames
    // 0..1_200. Two configuration edits before the wrap release the old
    // occurrence and then extend its carried suppression. Neither may outlive
    // the wrap: the next pass is a new onset and sounds under the new law.
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.edit.loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut looped = project(1.0, 0, 8 << 32).snapshot();
    looped.loop_region = Some(LoopRegion::new(0, 1_200, true).unwrap());
    let looped = legacy_project_from_snapshot(looped).unwrap();
    let states = [0, 1, 0];
    let cuts = [301, 601];
    let wrap = 1_200;
    let mut outputs = Vec::new();
    for block in [1, 64, 512] {
        let prepare = |state| {
            SessionPlayback::new(revision_with_instrument_registry(
                &with_test_instrument(&looped, definition.id, state),
                Arc::clone(&table),
                &registry,
            ))
            .unwrap()
        };
        let mut playback = prepare(states[0]);
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = Vec::new();
        let mut buffer = vec![[0.0; 2]; block];
        for (edit, cut) in cuts.iter().enumerate() {
            while playback.performance_frame() < *cut {
                let width = block.min(usize::try_from(cut - playback.performance_frame()).unwrap());
                playback.render(&mut buffer[..width]).unwrap();
                output.extend_from_slice(&buffer[..width]);
            }
            let mut candidate = prepare(states[edit + 1]);
            candidate.try_inherit_state_from(&mut playback).unwrap();
            assert!(
                candidate
                    .suppressions
                    .iter()
                    .all(|entry| entry.until_frame <= wrap),
                "block={block} edit={edit}: {:?}",
                candidate
                    .suppressions
                    .iter()
                    .map(|entry| entry.until_frame)
                    .collect::<Vec<_>>()
            );
            playback = candidate;
        }
        while playback.performance_frame() < wrap + 64 {
            let width =
                block.min(usize::try_from(wrap + 64 - playback.performance_frame()).unwrap());
            playback.render(&mut buffer[..width]).unwrap();
            output.extend_from_slice(&buffer[..width]);
        }
        // The held occurrence keeps its old law until the wrap, as unedited
        // playback would, and the next pass starts again under the last
        // configuration (the same law here): no doubled or silenced pass.
        let mut reference = prepare(states[0]);
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut expected = vec![[0.0; 2]; output.len()];
        for chunk in expected.chunks_mut(block) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(
            output[usize::try_from(wrap).unwrap()],
            [0.2; 2],
            "block={block}"
        );
        assert_eq!(output, expected, "block={block}");
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
}

#[test]
fn retained_arrangement_release_ends_when_a_later_edit_enables_a_loop() {
    use hex_lattice::NativeInstrumentDefinition;
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.edit.enable_loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let unlooped = project(1.0, 0, 8 << 32);
    let mut snapshot = unlooped.snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 1_200, true).unwrap());
    let looped = legacy_project_from_snapshot(snapshot).unwrap();
    for (candidate_state, expected) in [(0, 0.2), (1, 0.4)] {
        for block in [1, 64, 512] {
            let prepare = |project: &Project, state| {
                SessionPlayback::new(revision_with_instrument_registry(
                    &with_test_instrument(project, definition.id, state),
                    Arc::clone(&table),
                    &registry,
                ))
                .unwrap()
            };
            let mut old = prepare(&unlooped, 0);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut samples = vec![[0.0; 2]; 301];
            for chunk in samples.chunks_mut(block) {
                old.render(chunk).unwrap();
            }
            let mut changed = prepare(&unlooped, 1);
            changed.try_inherit_state_from(&mut old).unwrap();
            let mut samples = vec![[0.0; 2]; 300];
            for chunk in samples.chunks_mut(block) {
                changed.render(chunk).unwrap();
            }
            let mut next = prepare(&looped, candidate_state);
            next.try_inherit_state_from(&mut changed).unwrap();
            let mut samples = vec![[0.0; 2]; 600];
            for chunk in samples.chunks_mut(block) {
                next.render(chunk).unwrap();
            }
            assert_eq!(
                samples[599], [expected; 2],
                "block={block} state={candidate_state}"
            );
        }
    }
}

#[test]
fn arrangement_handoff_suppresses_through_candidate_release_end() {
    use hex_lattice::NativeInstrumentDefinition;

    let old_definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.short-release",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let new_definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.long-release",
        ),
        levels: [0.7, 0.9],
        release_frames: 512,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&old_definition, &new_definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 1 << 32);
    let old_project = with_test_instrument(&base, old_definition.id, 0);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    assert_eq!(prefix, [[0.2; 2]; 64]);

    let new_project = with_test_instrument(&base, new_definition.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &new_project,
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let old_voice_end = 24_000 + old_definition.release_frames;
    let candidate_occurrence_end = 24_000 + new_definition.release_frames;
    assert_eq!(candidate.releases.len(), 1);
    assert_eq!(candidate.releases[0].voice.end_frame(), old_voice_end);
    assert_eq!(candidate.suppressions.len(), 1);
    assert_eq!(
        candidate.suppressions[0].until_frame,
        candidate_occurrence_end
    );

    let start = candidate.performance_frame;
    let mut rest = vec![[0.0; 2]; (candidate_occurrence_end - start) as usize];
    candidate.render(&mut rest).unwrap();
    let after_old_voice = (old_voice_end - start) as usize;
    assert!(
        rest[..after_old_voice]
            .iter()
            .all(|frame| *frame == [0.2; 2])
    );
    assert!(
        rest[after_old_voice..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
}

#[test]
fn later_arrangement_handoffs_extend_suppression_after_old_voice_ends() {
    use hex_lattice::NativeInstrumentDefinition;

    let short = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-short",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let medium = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-medium",
        ),
        levels: [0.5, 0.6],
        release_frames: 256,
    };
    let long = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-long",
        ),
        levels: [0.7, 0.9],
        release_frames: 1_024,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 3] = [&short, &medium, &long];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 1 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, short.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let mut medium_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, medium.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    medium_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    let mut until_old_end = vec![[0.0; 2]; 24_020 - 64];
    medium_revision.render(&mut until_old_end).unwrap();
    assert!(medium_revision.releases.is_empty());
    assert_eq!(medium_revision.suppressions[0].until_frame, 24_256);

    let mut long_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, long.id, 0),
        table,
        &registry,
    ))
    .unwrap();
    long_revision
        .try_inherit_state_from(&mut medium_revision)
        .unwrap();
    assert!(long_revision.releases.is_empty());
    assert_eq!(long_revision.suppressions[0].until_frame, 25_024);
    let mut rest = vec![[0.0; 2]; 25_024 - 24_020];
    long_revision.render(&mut rest).unwrap();
    assert!(rest.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn reordered_multitrack_handoff_extends_carried_suppression_after_new_entry() {
    use hex_lattice::NativeInstrumentDefinition;

    let short = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-short",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let medium = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-medium",
        ),
        levels: [0.5, 0.6],
        release_frames: 256,
    };
    let long = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-long",
        ),
        levels: [0.7, 0.9],
        release_frames: 1_024,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 3] = [&short, &medium, &long];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let make_project = |first_type, second_type, reverse_tracks| {
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        let mut later_track = snapshot.tracks[0].clone();
        later_track.id = 20;
        later_track.name = "later track".into();
        if let Some(instrument) = &mut later_track.instrument {
            instrument.id = 21;
            instrument.state =
                hex_lattice_contract::NativeDeviceState::new(second_type, 1, vec![0]).unwrap();
        }
        let midi_clips = match &mut later_track.content {
            TrackContentSnapshot::Instrument(clips) => Some(clips),
            TrackContentSnapshot::Hybrid { midi_clips, .. } => Some(midi_clips),
            TrackContentSnapshot::Audio(_) => None,
        };
        if let Some(clip) = midi_clips.and_then(|clips| clips.first_mut()) {
            clip.id = 22;
            if let Some(note) = clip.notes.first_mut() {
                note.id = 23;
                note.duration_raw = 1 << 32;
            }
        }
        if let Some(instrument) = &mut snapshot.tracks[0].instrument {
            instrument.state =
                hex_lattice_contract::NativeDeviceState::new(first_type, 1, vec![0]).unwrap();
        }
        snapshot.next_id = 24;
        snapshot.tracks.push(later_track);
        if reverse_tracks {
            snapshot.tracks.reverse();
        }
        legacy_project_from_snapshot(snapshot).unwrap()
    };

    let initial = make_project(short.id, short.id, false);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &initial,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut start = [[0.0; 2]; 64];
    previous.render(&mut start).unwrap();

    // Track 1 is visited first and Track 20 second. This creates one carried
    // suppression for Track 8 while leaving Track 1's longer note unchanged.
    let first_edit = make_project(short.id, medium.id, false);
    let mut middle = SessionPlayback::new(revision_with_instrument_registry(
        &first_edit,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    middle.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(middle.suppressions.len(), 1);
    assert_eq!(middle.suppressions[0].track.value(), 20);
    let mut until_old_release_ends = vec![[0.0; 2]; 24_020 - 64];
    middle.render(&mut until_old_release_ends).unwrap();
    assert!(middle.releases.is_empty());

    // The candidate reorders tracks. The old route order still processes Track
    // 1 first, appending its new suppression after Track 8's carried entry;
    // extending Track 8 must search only the sorted carried prefix.
    let reordered_edit = make_project(medium.id, long.id, true);
    let mut final_revision = SessionPlayback::new(revision_with_instrument_registry(
        &reordered_edit,
        table,
        &registry,
    ))
    .unwrap();
    final_revision.try_inherit_state_from(&mut middle).unwrap();

    let first_suppression = final_revision
        .suppressions
        .iter()
        .find(|entry| entry.track.value() == 1)
        .unwrap();
    let later_suppression = final_revision
        .suppressions
        .iter()
        .find(|entry| entry.track.value() == 20)
        .unwrap();
    assert_eq!(first_suppression.track.value(), 1);
    assert_eq!(later_suppression.track.value(), 20);
    assert_eq!(later_suppression.until_frame, 25_024);
    assert_eq!(final_revision.suppressions.len(), 2);
}

#[test]
fn repeated_arrangement_instrument_edits_do_not_recapture_suppressed_occurrences() {
    use hex_lattice::NativeInstrumentDefinition;

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.repeat-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.repeat-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_block = [[0.0; 2]; 64];
    previous.render(&mut first_block).unwrap();

    let mut second_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, second.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    second_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    let mut second_block = [[0.0; 2]; 64];
    second_revision.render(&mut second_block).unwrap();

    let mut third_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        table,
        &registry,
    ))
    .unwrap();
    third_revision
        .try_inherit_state_from(&mut second_revision)
        .unwrap();
    assert_eq!(third_revision.releases.len(), 1);
    assert_eq!(
        third_revision.releases[0].timeline,
        ReleaseTimeline::Arrangement
    );
    let mut after_two_edits = [[0.0; 2]; 64];
    third_revision.render(&mut after_two_edits).unwrap();
    assert_eq!(after_two_edits, [[0.2; 2]; 64]);
}

#[test]
fn removing_track_after_arrangement_handoff_keeps_only_the_copied_voice() {
    use hex_lattice::NativeInstrumentDefinition;
    use hex_project::{Command, Editor};

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.remove-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.remove-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let second_project = with_test_instrument(&base, second.id, 0);
    let mut intermediate = SessionPlayback::new(revision_with_instrument_registry(
        &second_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    intermediate.try_inherit_state_from(&mut previous).unwrap();

    let track = second_project.tracks()[0].id();
    let mut editor = Editor::new(second_project, 8).unwrap();
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let mut removed = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        table,
        &registry,
    ))
    .unwrap();
    removed.try_inherit_state_from(&mut intermediate).unwrap();
    assert_eq!(removed.releases.len(), 1);
    let mut after_removal = [[0.0; 2]; 64];
    removed.render(&mut after_removal).unwrap();
    assert_eq!(after_removal, [[0.2; 2]; 64]);
}

#[test]
fn bypass_after_arrangement_handoff_does_not_recapture_the_old_occurrence() {
    use hex_lattice::NativeInstrumentDefinition;

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let second_project = with_test_instrument(&base, second.id, 0);
    let mut intermediate = SessionPlayback::new(revision_with_instrument_registry(
        &second_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    intermediate.try_inherit_state_from(&mut previous).unwrap();

    let bypassed_project = with_instrument_bypassed(&second_project, true);
    let mut bypassed = SessionPlayback::new(revision_with_instrument_registry(
        &bypassed_project,
        table,
        &registry,
    ))
    .unwrap();
    bypassed.try_inherit_state_from(&mut intermediate).unwrap();
    assert_eq!(bypassed.releases.len(), 1);
    let mut after_bypass = [[0.0; 2]; 64];
    bypassed.render(&mut after_bypass).unwrap();
    assert_eq!(after_bypass, [[0.2; 2]; 64]);
}

#[test]
fn arrangement_instrument_handoff_uses_new_sampler_for_exact_cut_onset() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.cut"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 1 << 32, 1 << 32);
    let old_project = with_test_instrument(&base, definition.id, 0);
    let changed_project = with_test_instrument(&base, definition.id, 1);
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut to_cut = vec![[0.0; 2]; 24_000];
    previous.render(&mut to_cut).unwrap();
    assert!(to_cut.iter().all(|frame| *frame == [0.0, 0.0]));
    assert_eq!(previous.performance_frame(), 24_000);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed_project,
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.release_voice_count(), 0);
    let mut at_cut = [[0.0; 2]; 1];
    candidate.render(&mut at_cut).unwrap();
    // The note begins exactly at the publication frame, so it belongs
    // exclusively to the candidate sampler.
    assert_eq!(at_cut[0], [0.4, 0.4]);
}

#[test]
fn mutable_handoff_refuses_pre_cut_note_on_new_track_then_allows_cleared_retry() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.new-track.chase"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(1.0, 1 << 32, 1 << 32);
    let mut previous = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 2];
    previous.render(&mut prefix).unwrap();
    assert_eq!(previous.performance_frame(), 2);

    let make_candidate = |arrangement_start: u64, session_start: u64| {
        let mut snapshot = original.snapshot();
        let old_track = snapshot.tracks[0].id;
        snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
        snapshot.tracks[0].instrument = None;
        snapshot.tracks[0].instrument_position = None;
        snapshot
            .session
            .cells
            .retain(|cell| cell.track != old_track);
        let track = 20;
        let instrument_id = 21;
        let arrangement_clip = 22;
        let arrangement_note = 23;
        let session_clip = 24;
        let session_note = 25;
        snapshot.tracks.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track,
            name: "new mutable track".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: hex_lattice_contract::NativeDeviceState::new(
                    definition.metadata.id,
                    1,
                    vec![0],
                )
                .unwrap(),
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: arrangement_clip,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    active: true,
                    id: arrangement_note,
                    start_raw: arrangement_start,
                    duration_raw: 1 << 32,
                    pitch: 64,
                    channel: 0,
                    velocity: 100,
                }],
            }]),
            sends: Vec::new(),
        });
        snapshot.session.cells.push(SessionCellSnapshot {
            track,
            scene: snapshot.session.scenes[0].id,
            content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                active: true,
                playback_offset_raw: 0,
                volume: hex_project::MidiClipVolume::default(),
                flavor: hex_project::ClipFlavor::default(),
                id: session_clip,
                name: "new mutable clip".into(),
                source: MidiSourceSnapshot {
                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                    source_length_raw: 32 << 32,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: 32 << 32,
                    looping: false,
                    notes: vec![MidiNoteSnapshot {
                        active: true,
                        id: session_note,
                        start_raw: session_start,
                        duration_raw: 1 << 32,
                        pitch: 64,
                        channel: 0,
                        velocity: 100,
                    }],
                },
            }),
        });
        snapshot.next_id = 26;
        legacy_project_from_snapshot(snapshot).unwrap()
    };

    let sounding = make_candidate(0, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &sounding,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    assert_eq!(candidate.routes.len(), 2);
    assert!(candidate.mutable_instruments[1].is_some());
    let mut candidate_span_active = false;
    candidate.revision.arrangement().tracks()[1]
        .for_each_mutable_midi_span(2, 3, |span| {
            candidate_span_active |= span.start < 2 && span.end > 2;
        })
        .unwrap();
    assert!(candidate_span_active);
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(previous.performance_frame(), 2);
    let old_track = previous.routes[0].id;
    assert_eq!(
        previous.owner(old_track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let cleared = make_candidate(1 << 32, 1 << 32);
    let mut retry = SessionPlayback::new(revision_with_instrument_registry(
        &cleared, table, &registry,
    ))
    .unwrap();
    retry.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(retry.performance_frame(), 2);
    assert_eq!(retry.release_voice_count(), 0);
}

#[test]
fn mutable_handoff_refuses_inherited_future_analytic_route_boundary() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.pending-boundary"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(1.0, 1 << 32, 1 << 32);
    let mut previous = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 1];
    previous.render(&mut prefix).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let pending = previous.routes[0].pending;
    assert!(pending.is_some_and(|entry| entry.frame > previous.performance_frame));

    let changed = with_test_instrument(&original, definition.metadata.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::MutableInstrumentRouteBoundary)
    );
    assert_eq!(previous.performance_frame(), 1);
    assert_eq!(previous.routes[0].pending, pending);
    let track = previous.routes[0].id;
    assert_eq!(
        previous.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    previous
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.performance_frame(), 1);
}

#[test]
fn arrangement_instrument_bypass_releases_old_voice_and_reenable_does_not_chase() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));
    let enabled = with_test_instrument(&base, definition.id, 0);
    let mut bypassed_snapshot = enabled.snapshot();
    bypassed_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    let bypassed = Project::from_legacy_snapshot(bypassed_snapshot).unwrap();

    let old_revision = revision_with_instrument_registry(&enabled, Arc::clone(&table), &registry);
    let mut previous = Playback::from_prepared(old_revision);
    previous.apply(PlaybackCommand::Play);
    let mut audible_prefix = [[0.0; 2]; 64];
    previous.render(&mut audible_prefix).unwrap();
    assert_eq!(audible_prefix, [[0.2; 2]; 64]);
    let candidate_revision =
        revision_with_instrument_registry(&bypassed, Arc::clone(&table), &registry);
    let mut candidate = Playback::from_prepared(candidate_revision);
    candidate.try_inherit_runtime_from(&mut previous).unwrap();
    let mut release = [[0.0; 2]; 10];
    candidate.render(&mut release).unwrap();
    assert_eq!(&release[..8], &[[0.2; 2]; 8]);
    assert_eq!(&release[8..], &[[0.0; 2]; 2]);

    let disabled_revision =
        revision_with_instrument_registry(&bypassed, Arc::clone(&table), &registry);
    let mut disabled = Playback::from_prepared(disabled_revision);
    disabled.apply(PlaybackCommand::Play);
    let mut silent_prefix = [[0.0; 2]; 64];
    disabled.render(&mut silent_prefix).unwrap();
    assert_eq!(silent_prefix, [[0.0; 2]; 64]);
    let enabled_revision = revision_with_instrument_registry(&enabled, table, &registry);
    let mut reenabled = Playback::from_prepared(enabled_revision);
    reenabled.try_inherit_runtime_from(&mut disabled).unwrap();
    let mut no_chase = vec![[0.0; 2]; 24_000 - 64];
    reenabled.render(&mut no_chase).unwrap();
    assert!(no_chase.iter().all(|frame| *frame == [0.0; 2]));
    let mut future_onset = [[0.0; 2]; 1];
    reenabled.render(&mut future_onset).unwrap();
    assert_eq!(future_onset[0], [0.2; 2]);
}

#[test]
fn reenabled_instrument_suppression_ends_at_the_loop_wrap_across_block_sizes() {
    use hex_lattice::NativeInstrumentDefinition;

    // Pulse #4124: the held note (beats 0..8) outlasts the loop over frames
    // 0..12_000. Re-enabling must not chase it, but the next pass sounds.
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.reenable.loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut looped = project(1.0, 0, 8 << 32).snapshot();
    looped.loop_region = Some(LoopRegion::new(0, 12_000, true).unwrap());
    let enabled = with_test_instrument(
        &legacy_project_from_snapshot(looped).unwrap(),
        definition.id,
        0,
    );
    let mut bypassed_snapshot = enabled.snapshot();
    bypassed_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    let bypassed = Project::from_legacy_snapshot(bypassed_snapshot).unwrap();
    let cut = 3_001;
    let mut outputs = Vec::new();
    for block in [1, 64, 512] {
        let mut disabled = SessionPlayback::new(revision_with_instrument_registry(
            &bypassed,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        disabled.apply(SessionPlaybackCommand::Play).unwrap();
        let mut buffer = vec![[0.0; 2]; block];
        let mut remaining = cut;
        while remaining > 0 {
            let width = block.min(remaining);
            disabled.render(&mut buffer[..width]).unwrap();
            remaining -= width;
        }
        let mut reenabled = SessionPlayback::new(revision_with_instrument_registry(
            &enabled,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        reenabled.try_inherit_state_from(&mut disabled).unwrap();
        let mut output = vec![[0.0; 2]; 12_000 - cut + 64];
        for chunk in output.chunks_mut(block) {
            reenabled.render(chunk).unwrap();
        }
        // No chase before the wrap; the next pass starts the note again.
        assert!(
            output[..12_000 - cut]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(output[12_000 - cut], [0.2; 2], "block={block}");
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
}

#[test]
fn arrangement_instrument_release_capacity_refusal_is_atomic_and_retryable() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.capacity.old"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.capacity.new"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));
    let old_project = with_test_instrument(&base, left.id, 0);
    let changed_project = with_test_instrument(&base, right.id, 0);
    let mut old = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut old_reference = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old_reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();
    old_reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    let changed_revision =
        revision_with_instrument_registry(&changed_project, Arc::clone(&table), &registry);
    let mut retry = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    let mut success = SessionPlayback::new(changed_revision).unwrap();
    retry.release_capacity = 0;
    assert_eq!(
        retry.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(old.performance_frame(), 64);
    assert_eq!(old.arrangement_frame(), 64);
    assert_eq!(old.release_voice_count(), 0);
    assert_eq!(old.suppressions.len(), 0);
    assert_eq!(retry.performance_frame(), 0);
    assert_eq!(retry.arrangement_frame(), 0);
    assert_eq!(retry.release_voice_count(), 0);
    assert_eq!(retry.suppressions.len(), 0);

    retry.release_capacity = 1;
    retry.try_inherit_state_from(&mut old).unwrap();
    success.try_inherit_state_from(&mut old_reference).unwrap();
    let mut retry_output = [[0.0; 2]; 64];
    let mut success_output = [[0.0; 2]; 64];
    retry.render(&mut retry_output).unwrap();
    success.render(&mut success_output).unwrap();
    assert_eq!(retry_output, success_output);
}

#[test]
fn session_state_and_type_changes_keep_old_voice_law_and_use_new_law_for_later_onsets() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());

    for (candidate_type, candidate_state, expected_future) in
        [(left.id, 1, 0.3_f32), (right.id, 0, 0.45_f32)]
    {
        let base = with_second_session_note(&session_only_project(&project(0.5, 0, 8 << 32)));
        let old_project = with_test_instrument(&base, left.id, 0);
        let old_revision =
            revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
        let clip = old_revision.session().midi_clips()[0].id;
        let mut previous = SessionPlayback::new(old_revision).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().all(|frame| *frame == [0.1, 0.1]));

        let changed = with_test_instrument(&base, candidate_type, candidate_state);
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            &changed,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut previous).unwrap();
        assert!(candidate.releases.iter().any(|entry| {
            entry.track_id == candidate.routes[0].id && entry.continuation.is_some()
        }));

        let mut immediate = [[0.0; 2]; 8];
        candidate.render(&mut immediate).unwrap();
        assert!(immediate.iter().all(|frame| *frame == [0.1, 0.1]));

        // The first onset under the candidate configuration is at beat one
        // (frame 24,000). It adds its new law while the captured old note keeps
        // the amplitude and gate authored by the previous revision.
        let mut until_future_note = vec![[0.0; 2]; 24_000 - 72];
        candidate.render(&mut until_future_note).unwrap();
        assert!(until_future_note.iter().all(|frame| *frame == [0.1, 0.1]));
        let mut at_future_note = [[0.0; 2]; 1];
        candidate.render(&mut at_future_note).unwrap();
        assert_eq!(at_future_note[0], [expected_future, expected_future]);
    }
}

#[test]
fn changed_instrument_does_not_rewrite_an_already_releasing_session_voice() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.release.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.release.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original =
        with_test_instrument(&session_only_project(&project(0.5, 0, 1 << 32)), left.id, 0);
    let old_revision = revision_with_instrument_registry(&original, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_release = vec![[0.0; 2]; 24_004];
    previous.render(&mut before_release).unwrap();

    let changed = with_test_instrument(&original, right.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let release = candidate
        .releases
        .iter()
        .find(|entry| entry.continuation.is_some())
        .expect("matching candidate preserves suppression for the old release");
    assert_eq!(release.voice.gate_end_frame(), 24_000);
    assert_eq!(release.voice.end_frame(), 24_008);
    assert_eq!(candidate.suppressions.len(), 1);

    let mut tail = [[0.0; 2]; 8];
    candidate.render(&mut tail).unwrap();
    assert_eq!(&tail[..4], &[[0.1, 0.1]; 4]);
    assert_eq!(&tail[4..], &[[0.0, 0.0]; 4]);
}

#[test]
fn instrument_edit_captures_only_the_changed_session_track() {
    use hex_lattice::NativeInstrumentDefinition;
    use hex_project::{Command, Created};

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.route.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.route.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "first".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("first MIDI track"),
    };
    let second = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "second".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("second MIDI track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("Session scene"),
    };
    for (track, pitch) in [(first, 60), (second, 64)] {
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "source".into(),
                source_length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::SessionClip(clip) => clip,
            _ => panic!("Session MIDI clip"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    for track in [first, second] {
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: registry.default_state(left.id).unwrap(),
            })
            .unwrap();
    }
    let old_project = editor.project().clone();
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let first_clip = old_revision
        .session()
        .midi_clips()
        .iter()
        .find(|clip| clip.track == first)
        .unwrap()
        .id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    assert!(prefix.iter().all(|frame| *frame == [0.4, 0.4]));

    let first_device = editor
        .project()
        .track(first)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: first_device,
            state: hex_lattice_contract::NativeDeviceState::new(left.id, 1, vec![1]).unwrap(),
        })
        .unwrap();
    let changed = editor.project().clone();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        candidate
            .releases
            .iter()
            .any(|entry| { entry.track_id == first && entry.continuation.is_some() })
    );
    assert!(
        !candidate
            .releases
            .iter()
            .any(|entry| entry.track_id == second)
    );
    assert!(
        candidate
            .suppressions
            .iter()
            .all(|entry| entry.track == first)
    );
    assert!(candidate.revision.session().midi_clip(first_clip).is_some());
    let first_route_index = candidate
        .routes
        .iter()
        .position(|route| route.id == first)
        .unwrap();
    let second_route_index = candidate
        .routes
        .iter()
        .position(|route| route.id == second)
        .unwrap();
    assert!(candidate.route_historical_floors[first_route_index].is_some());
    assert_eq!(candidate.route_historical_floors[second_route_index], None);
    let mut output = [[0.0; 2]; 64];
    candidate.render(&mut output).unwrap();
    assert!(
        output.iter().all(|frame| *frame == [0.4, 0.4]),
        "unrelated route changed across instrument edit: {:?}",
        (
            &output[..4],
            &candidate.releases,
            &candidate.suppressions,
            &candidate.routes
        )
    );

    let mut source_snapshot = old_project.snapshot();
    source_snapshot
        .tracks
        .iter_mut()
        .find(|track| track.id == first.value())
        .unwrap()
        .muted = true;
    let muted_source = legacy_project_from_snapshot(source_snapshot).unwrap();
    let mut changed_source_snapshot = muted_source.snapshot();
    let first_cell = changed_source_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == first.value())
        .unwrap();
    let SessionCellContentSnapshot::Midi(first_source) = &mut first_cell.content else {
        panic!("first route has Session MIDI")
    };
    first_source.source.notes[0].duration_raw = 2 << 32;
    let changed_source = legacy_project_from_snapshot(changed_source_snapshot).unwrap();
    let mut source_previous = SessionPlayback::new(revision_with_instrument_registry(
        &muted_source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let mut source_reference = SessionPlayback::new(revision_with_instrument_registry(
        &muted_source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    for playback in [&mut source_previous, &mut source_reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut source_prefix = [[0.0; 2]; 64];
    let mut source_reference_prefix = [[0.0; 2]; 64];
    source_previous.render(&mut source_prefix).unwrap();
    source_reference
        .render(&mut source_reference_prefix)
        .unwrap();
    assert_eq!(source_prefix, source_reference_prefix);
    assert!(source_prefix.iter().all(|frame| *frame == [0.2, 0.2]));
    let mut source_candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed_source,
        table,
        &registry,
    ))
    .unwrap();
    source_candidate
        .try_inherit_state_from(&mut source_previous)
        .unwrap();
    assert!(
        source_candidate
            .releases
            .iter()
            .any(|entry| { entry.track_id == second && entry.continuation.is_some() })
    );
    let mut source_actual = [[0.0; 2]; 64];
    let mut source_expected = [[0.0; 2]; 64];
    source_candidate.render(&mut source_actual).unwrap();
    source_reference.render(&mut source_expected).unwrap();
    assert_eq!(source_actual, source_expected);
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

#[test]
fn gain_only_handoff_preserves_route_clock_and_active_audio() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));

    let changed = project(0.75, 0, 8 << 32);
    let mut next = SessionPlayback::new(revision(&changed, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.performance_frame(), 64);
    assert_eq!(
        next.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session {
            clip: old.revision.session().midi_clips()[0].id
        })
    );
    let mut continued = [[0.0; 2]; 64];
    next.render(&mut continued).unwrap();
    assert!(continued.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn carried_voice_audio_is_partition_invariant_after_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    let mut expected = vec![[0.0; 2]; 513];
    whole.render(&mut expected).unwrap();
    let mut actual = Vec::new();
    let mut remaining = 513;
    for width in [1, 31, 256, 7, 218] {
        if remaining == 0 {
            break;
        }
        let width = width.min(remaining);
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
        remaining -= width;
    }
    assert_eq!(remaining, 0);
    assert_eq!(actual, expected);
}

#[test]
fn repeated_loop_occurrence_descriptor_and_pcm_survive_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![MidiNoteSnapshot {
            active: true,
            id: 6,
            start_raw: 5_u64 << 31, // loop phase 1/2; first repeat begins at 3.5
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 90,
        }];
    }
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = vec![[0.0; 2]; 100_000];
    old.render(&mut before_cut).unwrap();
    assert!(before_cut.iter().any(|frame| frame[0] != 0.0));

    let mut old_spans = MidiSpanBuffer::default();
    let origin = match old.routes[0].owner {
        RouteOwner::Session { origin, .. } => origin,
        _ => panic!("Session source is selected"),
    };
    old.revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query_with_origin(origin, &old.timing_view(), 100_000, 100_001, &mut old_spans)
        .unwrap();
    let repeated = old_spans
        .spans()
        .find(|span| span.note.value() == 6 && span.iteration == 1)
        .copied()
        .expect("first repeated occurrence remains held at the cut");
    assert_eq!(repeated.start_frame, 84_000);
    assert_eq!(repeated.gate_end_frame, 108_000);

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut slower_snapshot = slower.snapshot();
    slower_snapshot.session = original.snapshot().session;
    let slower = legacy_project_from_snapshot(slower_snapshot).unwrap();
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    assert!(whole.releases.iter().any(|entry| {
        entry
            .continuation
            .is_some_and(|continuation| continuation.key.iteration == 1)
    }));
    assert!(whole.suppressions.iter().any(|entry| entry.iteration == 1));

    let mut expected = [[0.0; 2]; 256];
    let mut actual = Vec::new();
    whole.render(&mut expected).unwrap();
    for width in [1, 31, 64, 127, 33] {
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
    }
    assert_eq!(actual.as_slice(), expected);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn source_edit_at_exact_cut_remains_future_eligible() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();

    let edited = with_session_note_id(&project(0.5, cut.raw(), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut at_cut = [[0.0; 2]; 64];
    next.render(&mut at_cut).unwrap();
    assert!(at_cut.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn inserted_strictly_historical_note_does_not_chase_after_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();
    let edited = with_session_note_id(&project(0.5, cut.raw().saturating_sub(1), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut after = [[0.0; 2]; 64];
    next.render(&mut after).unwrap();
    assert!(after.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn tempo_handoff_preserves_active_voice_phase_and_remaps_held_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 64 + offset as u64;
        let expected = table.sample(voice, absolute, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn tempo_handoff_keeps_already_releasing_voice_on_its_original_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = vec![[0.0; 2]; 6_100];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let old_gate = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1 << 30))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 6_100 + offset as u64;
        let expected = table.sample(voice, absolute, old_gate) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn stopping_after_handoff_cuts_carried_held_voice_without_requerying_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.release_voice_count(), 2); // arrangement and carried session voice

    next.apply(SessionPlaybackCommand::StopTrack {
        track,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Silence));
    assert_eq!(next.release_voice_count(), 2);
    assert!(
        next.releases
            .iter()
            .filter(|entry| entry.continuation.is_none())
            .any(|entry| entry.voice.gate_end_frame() == next.performance_frame)
    );
}

#[test]
fn seek_after_anchored_handoff_uses_base_clock_and_resets_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(next.anchor.is_some());
    next.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    assert_eq!(next.performance_frame(), 0);
    assert_eq!(next.arrangement_frame(), 0);
    assert!(next.anchor.is_none());
    assert!(next.historical_floor.is_none());
    assert!(next.releases.is_empty());
    assert!(next.suppressions.is_empty());
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let before = next.performance_frame();
    assert_eq!(
        next.apply(SessionPlaybackCommand::Seek(u64::MAX)),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(next.performance_frame(), before);
    assert!(next.anchor.is_none());
}

#[test]
fn pause_and_stop_after_handoff_hold_then_reset_the_reanchored_clock() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let prepared = revision(&original, Arc::clone(&table));
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut oracle = SessionPlayback::new(prepared).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    for runtime in [&mut old, &mut oracle] {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    oracle
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut candidate = SessionPlayback::new(revision(&slower, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.anchor.is_some());
    let before = candidate.performance_frame();
    candidate.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused = [[7.0; 2]; 32];
    candidate.render(&mut paused).unwrap();
    let mut released = [[0.0; 2]; 32];
    oracle.render(&mut released).unwrap();
    assert_eq!(
        paused, released,
        "reanchored historical gates release once at their original source age"
    );
    assert_eq!(candidate.performance_frame(), before);

    candidate.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.arrangement_frame(), 0);
    assert!(candidate.anchor.is_none());
    assert!(candidate.historical_floor.is_none());
    assert!(candidate.releases.is_empty());
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        matches!(candidate.routes[0].owner, RouteOwner::Session { origin, .. } if origin == SessionMidiOrigin::launch(BeatTime::ZERO))
    );
}

#[test]
fn failed_handoff_capacity_is_atomic_and_can_be_retried() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();
    assert!(block.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));
    let old_frame = old.performance_frame;
    let old_release_count = old.release_voice_count();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let candidate_revision = revision(&slower, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.try_inherit_state_from(&mut old).unwrap();
    candidate.release_capacity = 1;
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.release_voice_count(), 0);
    assert_eq!(old.performance_frame(), old_frame);
    assert_eq!(old.release_voice_count(), old_release_count);

    candidate.release_capacity = SESSION_RELEASE_CAPACITY;
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.performance_frame(), old_frame);
    assert!(candidate.release_voice_count() > 0);
    let mut after_refusal = [[0.0; 2]; 128];
    let mut twin_output = [[0.0; 2]; 128];
    candidate.render(&mut after_refusal).unwrap();
    twin.render(&mut twin_output).unwrap();
    assert_eq!(after_refusal, twin_output);
}

#[test]
fn failed_instrument_handoff_stages_route_floor_atomically_and_retries_pcm() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let replaced = with_replaced_instrument_id(&original);
    let old_revision = revision(&original, Arc::clone(&table));
    let candidate_revision = revision(&replaced, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;

    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut old_twin = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut old, &mut old_twin] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut twin_prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();
    old_twin.render(&mut twin_prefix).unwrap();
    assert_eq!(prefix, twin_prefix);

    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    let mut successful_twin = SessionPlayback::new(candidate_revision).unwrap();
    successful_twin
        .try_inherit_state_from(&mut old_twin)
        .unwrap();

    let old_routes = old.routes.clone();
    let old_release_facts = old
        .releases
        .iter()
        .map(|entry| {
            (
                entry.track_id,
                entry.location.track_index(),
                entry.order,
                entry.continuation.map(|continuation| {
                    (
                        continuation.key.clip,
                        continuation.key.note,
                        continuation.key.iteration,
                    )
                }),
                entry.voice.gate_end_frame(),
                entry.voice.end_frame(),
            )
        })
        .collect::<Vec<_>>();
    let old_suppressions = old.suppressions.clone();
    let old_clock = (
        old.performance_frame,
        old.arrangement_frame,
        old.anchor,
        old.historical_floor,
        old.route_historical_floors,
        old.playing,
    );
    candidate.release_capacity = 0;
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(candidate.routes[0].owner, RouteOwner::Arrangement);
    assert_eq!(candidate.performance_frame, 0);
    assert_eq!(candidate.route_historical_floors, [None; MAX_TRACKS]);
    assert!(candidate.releases.is_empty());
    assert!(candidate.suppressions.is_empty());
    assert_eq!(old.routes, old_routes);
    assert_eq!(
        old.releases
            .iter()
            .map(|entry| {
                (
                    entry.track_id,
                    entry.location.track_index(),
                    entry.order,
                    entry.continuation.map(|continuation| {
                        (
                            continuation.key.clip,
                            continuation.key.note,
                            continuation.key.iteration,
                        )
                    }),
                    entry.voice.gate_end_frame(),
                    entry.voice.end_frame(),
                )
            })
            .collect::<Vec<_>>(),
        old_release_facts
    );
    assert_eq!(old.suppressions, old_suppressions);
    assert_eq!(
        (
            old.performance_frame,
            old.arrangement_frame,
            old.anchor,
            old.historical_floor,
            old.route_historical_floors,
            old.playing,
        ),
        old_clock
    );

    candidate.release_capacity = SESSION_RELEASE_CAPACITY;
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.route_historical_floors[0].is_some());
    assert_eq!(
        candidate.route_historical_floors,
        successful_twin.route_historical_floors
    );
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    candidate.render(&mut actual).unwrap();
    successful_twin.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn repeated_handoff_keeps_releasing_voice_when_new_gate_is_unrepresentable() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut near_gate = vec![[0.0; 2]; 6_100];
    old.render(&mut near_gate).unwrap(); // voice is already releasing

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut middle = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    middle.try_inherit_state_from(&mut old).unwrap();
    let second_slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(1_000_000).unwrap());
    let mut final_revision = SessionPlayback::new(revision(&second_slower, table)).unwrap();
    let cut_beat = middle
        .timing_view()
        .frame_to_beat(middle.performance_frame())
        .unwrap();
    let base_beat = base_timing_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
    )
    .frame_to_beat(middle.performance_frame())
    .unwrap();
    let final_timing = timing_view_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
        Some(SessionClockAnchor {
            cut_frame: middle.performance_frame(),
            old_beat: cut_beat,
            new_beat: base_beat,
        }),
    );
    assert!(
        final_timing
            .beat_to_frame(BeatTime::from_raw(1 << 30))
            .is_err()
    );
    final_revision.try_inherit_state_from(&mut middle).unwrap();
    assert!(final_revision.release_voice_count() > 0);
}

#[test]
fn quantized_same_clip_retrigger_after_handoff_clears_old_occurrence_suppression() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut playback = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    playback.try_inherit_state_from(&mut old).unwrap();
    assert!(!playback.suppressions.is_empty());
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due = playback.routes[0].pending.unwrap().frame;
    let width = usize::try_from(due - playback.performance_frame + 64).unwrap();
    let mut output = vec![[0.0; 2]; width];
    playback.render(&mut output).unwrap();
    assert_eq!(playback.performance_frame, due + 64);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(playback.suppressions.is_empty()); // outgoing occurrence key was cleared
    assert!(output[width - 64..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn repeated_tempo_handoffs_keep_historical_voice_without_requerying_onset() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warm = [[0.0; 2]; 64];
    first.render(&mut warm).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(180_000_000).unwrap());
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, 64 + offset as u64, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn repeated_handoffs_suppress_an_occurrence_exactly_at_the_previous_cut() {
    let table = Arc::new(hex_devices::SineTable::new());
    let baseline = project(0.5, 0, 8 << 32);
    let baseline_revision = revision(&baseline, Arc::clone(&table));
    let cut_beat = baseline_revision
        .session()
        .timing()
        .frame_to_beat(64)
        .unwrap();
    assert_eq!(
        baseline_revision
            .session()
            .timing()
            .beat_to_frame(cut_beat)
            .unwrap(),
        64
    );
    let original = project(0.5, cut_beat.raw(), 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = [[0.0; 2]; 64];
    first.render(&mut before_cut).unwrap();

    let slower = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(60_000_000).unwrap(),
    );
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(180_000_000).unwrap(),
    );
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(cut_beat.raw() + (8 << 32)))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, offset as u64, gate - 64) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn handoff_remaps_pending_quantized_target_by_beat_and_cancels_missing_clip() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let target = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(4 << 32))
        .unwrap();
    assert!(target > next.performance_frame());
    let mut until = vec![[0.0; 2]; (target - next.performance_frame()) as usize];
    next.render(&mut until).unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Arrangement));
    let mut at_target = [[0.0; 2]; 1];
    next.render(&mut at_target).unwrap();
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    let mut snapshot = original.snapshot();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(report.cancelled_pending_tracks(), &[Some(track)]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn pending_scene_launch_keeps_its_resolved_clip_when_scene_contents_change() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchScene {
        scene: old.revision.session().scenes()[0].id,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(
        old.routes[0].pending.unwrap().target,
        RouteTarget::Session(clip)
    );

    let mut snapshot = original.snapshot();
    snapshot.session.scenes.push(SceneSnapshot {
        id: 7,
        name: "moved clip".into(),
    });
    snapshot.session.cells[0].scene = 7;
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 1,
        scene: 4,
        content: SessionCellContentSnapshot::Stop,
    });
    let edited = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(report.cancelled_pending_tracks().is_empty());
    let route = candidate
        .routes
        .iter()
        .find(|route| route.id == track)
        .unwrap();
    assert_eq!(route.pending.unwrap().target, RouteTarget::Session(clip));
}

#[test]
fn due_pending_quantization_uses_actual_cut_beat_on_shared_frame_plateau() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old_snapshot = project(0.5, 0, 8 << 32).snapshot();
    old_snapshot.sample_rate = 48_000;
    let boundary = 4_u64 << 32;
    let later_same_frame = boundary + 80_000;
    old_snapshot.tempo_points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
        TempoPoint::new(
            BeatTime::from_raw(later_same_frame),
            Tempo::new(60_000_000).unwrap(),
        ),
    ];
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Seek(95_999)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(old.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut at_cut = [[0.0; 2]; 1];
    old.render(&mut at_cut).unwrap();
    assert_eq!(old.performance_frame(), 96_000);
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    let effective_cut = old.timing_view().frame_to_beat(96_000).unwrap();
    assert!(effective_cut.raw() > boundary);
    assert_eq!(
        old.timing_view().beat_to_frame(effective_cut).unwrap(),
        96_000
    );

    let mut new_snapshot = old_project.snapshot();
    new_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let new_project = legacy_project_from_snapshot(new_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&new_project, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(candidate.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut launch = [[0.0; 2]; 1];
    candidate.render(&mut launch).unwrap();
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    let RouteOwner::Session { origin, .. } = candidate.routes[0].owner else {
        panic!("due launch selects the Session clip");
    };
    assert_eq!(origin, SessionMidiOrigin::launch(effective_cut));
    let mut spans = MidiSpanBuffer::default();
    candidate
        .revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query_with_origin(origin, &candidate.timing_view(), 96_000, 96_001, &mut spans)
        .unwrap();
    assert!(spans.spans().any(|span| span.start_frame == 96_000));
}

#[test]
fn later_chunk_capacity_refusal_rolls_back_carried_voice_and_retries_against_twin() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(600_000_000).unwrap());
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: 13,
            start_raw: 1_u64 << 27, // begins 1/32 beat after the carried note
            duration_raw: 1 << 30,
            pitch: 67,
            channel: 0,
            velocity: 90,
        });
    }
    // Keep the Arrangement-only note outside the handoff window so launching
    // Session does not create a second, unrelated release-bank entry.
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].notes[0].start_raw = 8 << 32;
    }
    snapshot.next_id = 14;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup = [[0.0; 2]; 1];
    old.render(&mut warmup).unwrap();

    let mut changed_snapshot = old_project.snapshot();
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = revision(&changed, Arc::clone(&table));
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    failed.routes[0].pending = Some(PendingRoute {
        frame: 257,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
        current_reservation: None,
    });

    let mut prefix_probe = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    prefix_probe.release_capacity = 1;
    prefix_probe.try_inherit_state_from(&mut old).unwrap();
    prefix_probe.routes[0].pending = failed.routes[0].pending;
    let mut first_chunk = [[0.0; 2]; 256];
    prefix_probe.render(&mut first_chunk).unwrap();
    assert!(first_chunk.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(prefix_probe.performance_frame(), 257);

    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.release_capacity = 1;
    twin.try_inherit_state_from(&mut old).unwrap();
    twin.routes[0].pending = failed.routes[0].pending;

    let routes = failed.routes.clone();
    let releases = failed.releases.clone();
    let suppressions = failed.suppressions.clone();
    let release_order = failed.release_order;
    let route_generation = failed.route_state_generation();
    let mut whole = [[9.0; 2]; 512];
    assert_eq!(
        failed.render(&mut whole),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(whole, [[0.0; 2]; 512]);
    assert_eq!(failed.performance_frame(), 1);
    assert_eq!(failed.arrangement_frame(), 1);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_order, release_order);
    assert_eq!(failed.releases.len(), releases.len());
    assert_eq!(failed.suppressions, suppressions);
    assert_eq!(
        failed.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(failed.routes[0].pending, routes[0].pending);
    assert_eq!(
        failed.releases[0].voice.end_frame(),
        releases[0].voice.end_frame()
    );

    failed
        .apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    twin.apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    let mut retry = [[0.0; 2]; 512];
    let mut expected = [[0.0; 2]; 512];
    failed.render(&mut retry).unwrap();
    twin.render(&mut expected).unwrap();
    assert_eq!(retry, expected);
}

#[test]
fn streamed_later_chunk_refusal_zeros_block_and_rolls_back_after_page_requests() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut original_snapshot = mixed_audio_midi_project(5, 18).snapshot();
    let original_note_id = original_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(original_clip)) = original_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    original_clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: original_note_id,
        start_raw: 0,
        duration_raw: 1 << 30,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    original_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut original_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    let original = legacy_project_from_snapshot(original_snapshot).unwrap();
    let stream_asset = original
        .assets()
        .find(|asset| asset.id().value() == 4)
        .unwrap();
    let stream_source = StreamedSourceMetadata::new(
        StreamSourceKey::new(71, stream_asset.id()),
        stream_asset.source_sample_rate(),
        original.sample_rate(),
        stream_asset.source_frames(),
        stream_asset.playback_frames(),
        stream_asset.conversion_version(),
        [0x71; 32],
    )
    .unwrap();
    let make_sources = |project: &Project| {
        let resident = crate::Audio::new(48_000, vec![[0.25, -0.25]; 32]).unwrap();
        project
            .assets()
            .map(|asset| {
                let source = if asset.id().value() == 4 {
                    PreparedAudioSource::Streamed(Arc::new(stream_source.clone()))
                } else {
                    PreparedAudioSource::Resident(resident.clone())
                };
                (asset.id(), source)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let old_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &original,
            &make_sources(&original),
            71,
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut provider_refused = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    provider_refused
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut refused_output = [[0.0; 2]; 1];
    assert_eq!(
        provider_refused.render(&mut refused_output),
        Err(SessionPlaybackError::MediaPagesUnavailable)
    );
    let mut unused_predecessor = SessionPlayback::new(old_revision).unwrap();
    assert_eq!(
        provider_refused.try_inherit_state_from(&mut unused_predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
    let midi_clip = old
        .revision
        .session()
        .midi_clips()
        .iter()
        .find(|clip| clip.track.value() == 6)
        .unwrap()
        .id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: midi_clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup_pages = ContinuityMissingPages;
    let mut warmup = [[0.0; 2]; 1];
    old.render_with_pages(&mut warmup, &mut warmup_pages)
        .unwrap();

    let mut changed_snapshot = original.snapshot();
    let added_note_id = changed_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(clip)) = changed_snapshot
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
        id: added_note_id,
        start_raw: 1_u64 << 27,
        duration_raw: 1 << 30,
        pitch: 67,
        channel: 0,
        velocity: 90,
    });
    changed_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut changed_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &changed,
            &make_sources(&changed),
            71,
            Arc::clone(&table),
            Some(old.revision.session()),
        )
        .unwrap(),
    );
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    let audio_clip = failed.revision.session().audio_clips()[0].id;
    failed
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    failed.routes[1].pending = Some(PendingRoute {
        frame: 256,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
        current_reservation: None,
    });

    #[derive(Default)]
    struct MissingPages(Vec<(StreamSourceKey, u64)>);
    impl MediaPages for MissingPages {
        fn page(
            &mut self,
            _key: StreamSourceKey,
            _page_index: u64,
        ) -> Option<BorrowedMediaPage<'_>> {
            None
        }

        fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.0.push((key, page_index));
        }
    }

    let routes = failed.routes.clone();
    let release_end = failed.releases[0].voice.end_frame();
    let route_generation = failed.route_state_generation();
    // The 500 bpm edit remaps the Arrangement cut by beat (Pulse #3935).
    let arrangement_frame = failed.arrangement_frame();
    let mut pages = MissingPages::default();
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        failed.render_with_pages(&mut output, &mut pages),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(pages.0, [(stream_source.key(), 0)]);
    assert_eq!(failed.performance_frame(), 1);
    assert_eq!(failed.arrangement_frame(), arrangement_frame);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_voice_count(), 1);
    assert_eq!(failed.releases[0].voice.end_frame(), release_end);
}

#[test]
fn deleting_the_last_track_keeps_its_copied_session_voice_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
    assert!(before.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));
    let mut snapshot = original.snapshot();
    snapshot.tracks.clear();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    assert_eq!(candidate.performance_frame(), old.performance_frame());
    assert_eq!(candidate.arrangement_frame(), old.arrangement_frame());
    candidate
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    candidate
        .apply(SessionPlaybackCommand::StopAllClips {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    let mut tail = [[0.0; 2]; 64];
    candidate.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn reordered_tracks_keep_project_order_and_deleted_arrangement_tail_uses_old_mix() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    snapshot.next_id = 15;
    for (track_id, clip_id, note_id, gain) in [(7, 13, 14, 0.25), (10, 11, 12, 0.75)] {
        snapshot.tracks.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("track {track_id}"),
            gain,
            balance: 0.2,
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
                id: clip_id,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    active: true,
                    id: note_id,
                    start_raw: 0,
                    duration_raw: 8 << 32,
                    pitch: 60 + track_id as u8,
                    channel: 0,
                    velocity: 90,
                }],
            }]),
            sends: Vec::new(),
        });
    }
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut edited = original.snapshot();
    edited.tracks = vec![edited.tracks[1].clone(), edited.tracks[0].clone()];
    let candidate_project = legacy_project_from_snapshot(edited).unwrap();
    let candidate_revision = revision(&candidate_project, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].id.value(), 7);
    assert_eq!(candidate.routes[1].id.value(), 1);
    let deleted_tail = candidate
        .releases
        .iter()
        .find(|entry| entry.track_id.value() == 10)
        .copied()
        .unwrap();

    let mut actual = [[0.0; 2]; 64];
    candidate.render(&mut actual).unwrap();
    let mut expected = [[0.0; 2]; 64];
    for track in candidate_revision.arrangement().tracks() {
        track
            .render_additive(candidate.arrangement_frame - 64, &mut expected)
            .unwrap();
    }
    for (offset, frame) in expected.iter_mut().enumerate() {
        let absolute = candidate.performance_frame - 64 + offset as u64;
        if let Some(sample) = deleted_tail.voice.sample_at(
            absolute,
            candidate_revision.arrangement().shared_contexts(),
            &candidate.retained_contexts,
        ) {
            deleted_tail.mix.add_frame(frame, sample);
        }
    }
    candidate_revision
        .arrangement()
        .master_mix()
        .apply(&mut expected);
    assert_eq!(actual, expected);
}

#[test]
fn source_start_and_loop_edit_releases_active_intro_occurrence_without_reusing_phase_key() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        let clip = &mut clips[0];
        clip.source_start_raw = 1 << 32;
        clip.loop_start_raw = 2 << 32;
        clip.loop_end_raw = 4 << 32;
        clip.looping = true;
        clip.notes = vec![
            MidiNoteSnapshot {
                active: true,
                id: 3,
                start_raw: 3_u64 << 31, // intro onset, gate crosses the first loop seam
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                active: true,
                id: 13,
                start_raw: 5_u64 << 31, // repeated loop phase
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
            },
        ];
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![
            MidiNoteSnapshot {
                active: true,
                id: 14,
                start_raw: 3_u64 << 31,
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
                active: true,
                id: 15,
                start_raw: 5_u64 << 31,
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
            },
        ];
    }
    snapshot.next_id = 16;
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut intro = vec![[0.0; 2]; 24_000];
    old.render(&mut intro).unwrap();

    let mut edited_snapshot = original.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut edited_snapshot.tracks[0].content
    {
        clips[0].source_start_raw = 0;
        clips[0].loop_start_raw = 2 << 32;
        clips[0].loop_end_raw = 5 << 32;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut edited_snapshot.session.cells[0].content {
        clip.source.source_start_raw = 0;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 5 << 32;
    }
    let edited = legacy_project_from_snapshot(edited_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        candidate
            .releases
            .iter()
            .any(|entry| entry.continuation.is_none() && entry.voice.end_frame() > 24_000)
    );
}

#[test]
fn moving_selected_clip_to_another_track_releases_old_route() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let old_track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
    assert!(before.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));

    let mut snapshot = original.snapshot();
    snapshot.tracks.push(TrackSnapshot {
        kind: hex_project::TrackKind::Ordinary,
        parent_group: None,
        color: hex_project::ElementColor::default(),
        gain_envelope: Vec::new(),
        gain_touch_spans: Vec::new(),
        gain_touch_epoch: 0,
        id: 7,
        name: "other".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Instrument(Vec::new()),
        sends: Vec::new(),
    });
    snapshot.session.cells[0].track = 7;
    let moved = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&moved, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(candidate.release_voice_count(), 2);
}

#[test]
fn unrepresentable_matching_gate_remap_refuses_without_candidate_commit() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].length_raw = u64::MAX;
        clips[0].source_length_raw = u64::MAX;
        clips[0].loop_end_raw = u64::MAX;
        clips[0].notes[0].duration_raw = u64::MAX;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_length_raw = u64::MAX;
        clip.source.loop_end_raw = u64::MAX;
        clip.source.notes[0].duration_raw = u64::MAX;
    }
    let long = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&long, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let cut = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1_u64 << 62))
        .unwrap();
    old.performance_frame = cut;
    old.arrangement_frame = cut;

    let mut faster_snapshot = long.snapshot();
    faster_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(240_000_000).unwrap(),
    )];
    let faster = legacy_project_from_snapshot(faster_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&faster, table)).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(candidate.release_voice_count(), 0);
}

#[test]
fn reduced_shared_bank_refuses_a_cut_atomically_and_recovers_after_expiry() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let mut playback = SessionPlayback::new(revision(&project, table)).unwrap();
    let track = playback.routes[0].id;
    let clip = playback.revision.session().midi_clips()[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial = [[0.0; 2]; 2_500];
    playback.render(&mut initial).unwrap();
    playback.release_capacity = 1;
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.release_voice_count(), 1);
    let old_release_boundaries = (
        playback.releases[0].voice.gate_end_frame(),
        playback.releases[0].voice.end_frame(),
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    // A full bank still permits cancelling an unheard exact-onset occurrence.
    let unheard_frame = playback.performance_frame();
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.performance_frame(), unheard_frame);
    assert_eq!(playback.release_voice_count(), 1);
    assert_eq!(
        (
            playback.releases[0].voice.gate_end_frame(),
            playback.releases[0].voice.end_frame(),
        ),
        old_release_boundaries
    );
    assert_eq!(playback.owner(track), Some(SessionPlaybackOwner::Silence));
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    // Once that occurrence has sounded, its tail needs another release slot.
    let mut relaunched = [[0.0; 2]; 1];
    playback.render(&mut relaunched).unwrap();
    let cut_frame = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(playback.performance_frame(), cut_frame);
    assert_eq!(playback.release_voice_count(), 1);
    assert_eq!(
        (
            playback.releases[0].voice.gate_end_frame(),
            playback.releases[0].voice.end_frame(),
        ),
        old_release_boundaries
    );
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut after_expiry = [[0.0; 2]; 2_500];
    playback.render(&mut after_expiry).unwrap();
    assert_eq!(playback.release_voice_count(), 0);
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.owner(track), Some(SessionPlaybackOwner::Silence));
}

#[test]
fn route_status_generation_tracks_committed_commands_and_due_transitions() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let revision = revision(&project, Arc::clone(&table));
    let track = project.tracks()[0].id();
    let clip = match project.session_cells()[0].content() {
        hex_project::SessionCellContent::Midi(clip) => clip.id(),
        hex_project::SessionCellContent::Audio(_) => panic!("expected Session MIDI clip"),
        hex_project::SessionCellContent::Stop => panic!("expected Session clip"),
    };
    let mut playback = SessionPlayback::new(revision).unwrap();
    let initial = playback.route_state_generation();
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.track, track);
    assert_eq!(route.owner, SessionPlaybackOwner::Arrangement);
    assert_eq!(route.active_origin, None);
    assert_eq!(route.pending, None);

    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert_eq!(playback.route_state_generation(), initial);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let launched = playback.route_state_generation();
    assert_eq!(launched, initial.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.owner, SessionPlaybackOwner::Session { clip });
    assert_eq!(
        route.active_origin,
        Some(SessionMidiOrigin::launch(BeatTime::ZERO))
    );

    let mut advance = [[0.0; 2]; 64];
    playback.render(&mut advance).unwrap();
    let before_pending = playback.route_state_generation();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let pending_generation = playback.route_state_generation();
    assert_eq!(pending_generation, before_pending.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    let pending = route.pending.unwrap();
    assert_eq!(pending.target, SessionRouteTarget::Session { clip });
    assert_eq!(pending.performance_frame, 96_000);
    assert_eq!(pending.beat, BeatTime::from_quarters(4).unwrap());

    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.route_state_generation(),
        pending_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due_generation = playback.route_state_generation();
    let mut until_due = vec![[0.0; 2]; 96_000 - 64];
    playback.render(&mut until_due).unwrap();
    let mut first_due_block = [[0.0; 2]; 1];
    playback.render(&mut first_due_block).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        due_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    let before_stop = playback.route_state_generation();
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        before_stop.wrapping_add(1)
    );
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.route_status(0).unwrap().active_origin,
        Some(SessionMidiOrigin::launch(BeatTime::ZERO))
    );
}

#[test]
fn session_audio_matches_direct_frame_oracle_over_irregular_blocks_and_ends_silently() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.013 - 0.17, 0.31 - index as f32 * 0.007])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
    let media = audio_media(&project, &audio, &other);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = prepared.session().audio_clips()[0].id;
    let mut playback = SessionPlayback::new(prepared).unwrap();
    let track = playback.routes[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut rendered = Vec::new();
    for width in [1, 5, 2, 7, 4, 9, 3] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "source-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_seek_pause_stop_scene_stop_and_return_are_explicit_and_keep_midi_routes() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.011 + 0.1, 0.8 - index as f32 * 0.009])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = mixed_audio_midi_project(5, 18);
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
    let audio_clip = revision.session().audio_clips()[0].id;
    let midi_clip = revision.session().midi_clips()[0].id;
    let audio_track = revision.session().audio_clips()[0].track;
    let midi_track = revision.session().midi_clips()[0].track;
    let scene = revision.session().scenes()[0].id;
    let stop_scene = revision.session().scenes()[1].id;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 3];
    playback.render(&mut first).unwrap();
    assert_eq!(first[0], source[5]);
    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    let position = playback.performance_frame();
    let mut paused = [[1.0; 2]; 2];
    playback.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 2]);
    assert_eq!(playback.performance_frame(), position);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut resumed = [[0.0; 2]; 1];
    playback.render(&mut resumed).unwrap();
    assert_eq!(resumed[0], source[8]);
    playback.apply(SessionPlaybackCommand::Seek(100)).unwrap();
    let mut seeked = [[0.0; 2]; 1];
    playback.render(&mut seeked).unwrap();
    assert_eq!(seeked[0], source[5]);
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut restarted = [[0.0; 2]; 1];
    playback.render(&mut restarted).unwrap();
    assert_eq!(restarted[0], source[5]);
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: stop_scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn session_audio_midi_and_arrangement_routes_mix_additively_against_independent_renders() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [0.01 * (index as f32 + 1.0), -0.006 * (index as f32 + 2.0)])
        .collect();
    let audio = crate::Audio::new(48_000, source).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = session_audio_midi_arrangement_mix_project();
    let media = audio_media(&project, &audio, &other);
    let render = |launch_audio: bool, launch_midi: bool| {
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let audio_clip = revision.session().audio_clips()[0].id;
        let midi_clip = revision.session().midi_clips()[0].id;
        let mut playback = SessionPlayback::new(revision).unwrap();
        if launch_audio {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: audio_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        if launch_midi {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: midi_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 32];
        playback.render(&mut output).unwrap();
        output
    };
    let both = render(true, true);
    let audio_only = render(true, false);
    let midi_only = render(false, true);
    let arrangement_only = render(false, false);
    for index in 0..both.len() {
        for channel in 0..2 {
            let expected = audio_only[index][channel] + midi_only[index][channel]
                - arrangement_only[index][channel];
            assert!(
                (both[index][channel] - expected).abs() <= 1.0e-6,
                "frame {index} channel {channel}: actual {}, independent sum {expected}",
                both[index][channel]
            );
        }
    }
    assert!(both.iter().any(|frame| frame[0].abs() > 0.02));
}

#[test]
fn session_audio_quantization_cancel_and_tempo_handoff_keep_frame_domain() {
    let source_a: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 1.0, -(index as f32 + 2.0)])
        .collect();
    let source_b: Vec<Frame> = (0..32)
        .map(|index| [200.0 + index as f32, -400.0 - index as f32])
        .collect();
    let audio_a = crate::Audio::new(48_000, source_a.clone()).unwrap();
    let audio_b = crate::Audio::new(48_000, source_b.clone()).unwrap();
    let base = audio_project(5, 18, 4);
    let media = audio_media(&base, &audio_a, &audio_b);
    let mut playback = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(&base, &media, Arc::new(hex_devices::SineTable::new()), None)
            .unwrap(),
    ))
    .unwrap();
    let clip = playback.revision.session().audio_clips()[0].id;
    let track = playback.routes[0].id;
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial_frame = [[0.0; 2]; 1];
    playback.render(&mut initial_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(playback.route_status(0).unwrap().pending.is_some());
    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut prefix = [[0.0; 2]; 5];
    playback.render(&mut prefix).unwrap();

    let mut replacement_snapshot = base.snapshot();
    replacement_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut replacement_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.asset = 5;
    audio_clip.source_start = 2;
    audio_clip.source_end = 20;
    let replacement = legacy_project_from_snapshot(replacement_snapshot).unwrap();
    let replacement_media = audio_media(&replacement, &audio_a, &audio_b);
    let candidate_revision = Arc::new(
        PreparedRevision::prepare(
            &replacement,
            &replacement_media,
            Arc::clone(playback.revision.session().sine_table()),
            Some(playback.revision.session()),
        )
        .unwrap(),
    );
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut playback).unwrap();
    let mut next = [[0.0; 2]; 1];
    candidate.render(&mut next).unwrap();
    assert_eq!(next[0], source_b[7]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(candidate.performance_frame(), 7);

    let mut short_snapshot = replacement.snapshot();
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut short_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.source_start = 2;
    audio_clip.source_end = 4;
    let short = legacy_project_from_snapshot(short_snapshot).unwrap();
    let short_media = audio_media(&short, &audio_a, &audio_b);
    let short_revision = Arc::new(
        PreparedRevision::prepare(
            &short,
            &short_media,
            Arc::clone(candidate.revision.session().sine_table()),
            Some(candidate.revision.session()),
        )
        .unwrap(),
    );
    let mut shortened = SessionPlayback::new(short_revision).unwrap();
    shortened.try_inherit_state_from(&mut candidate).unwrap();
    let mut silent = [[1.0; 2]; 3];
    shortened.render(&mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 3]);
    assert_eq!(
        shortened.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_quantized_launch_begins_on_the_due_performance_frame() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 0.25, -((index as f32) + 0.5)])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
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
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_frame = [[0.0; 2]; 1];
    playback.render(&mut first_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
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
        96_000
    );

    let mut before_due = vec![[1.0; 2]; 96_000 - 1];
    playback.render(&mut before_due).unwrap();
    assert!(before_due.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(playback.performance_frame(), 96_000);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let mut rendered = Vec::new();
    for width in [1, 3, 2, 5, 4, 7] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "due-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn quantized_scene_44100_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(44100, false);
}

#[test]
fn quantized_scene_44100_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(44100, true);
}

#[test]
fn quantized_scene_48000_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(48000, false);
}

#[test]
fn quantized_scene_48000_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(48000, true);
}

#[test]
fn quantized_scene_96000_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(96000, false);
}

#[test]
fn quantized_scene_96000_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(96000, true);
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

#[test]
fn prepared_session_audio_refuses_missing_rate_or_length_mismatched_pcm_and_reuses_shared_pcm() {
    let project = audio_project(1, 7, 4);
    let audio = crate::Audio::new(48_000, vec![[0.2, -0.3]; 32]).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.4, -0.5]; 32]).unwrap();
    let media = audio_media(&project, &audio, &other);
    let first = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
    .unwrap();
    let second = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        second
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    let changed_pcm = crate::Audio::new(48_000, vec![[9.0, -9.0]; 32]).unwrap();
    let changed_media = audio_media(&project, &changed_pcm, &other);
    let changed = PreparedSessionInventory::prepare_with_media(
        &project,
        &changed_media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        !changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    assert_eq!(
        changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .resident_audio()
            .unwrap()
            .frames()[0],
        [9.0, -9.0]
    );
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let wrong_rate = crate::Audio::new(44_100, vec![[0.0; 2]; 32]).unwrap();
    let bad_media = audio_media(&project, &wrong_rate, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_media,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let short_pcm = crate::Audio::new(48_000, vec![[0.0; 2]; 31]).unwrap();
    let bad_length = audio_media(&project, &short_pcm, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_length,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
}

#[test]
fn used_or_accepted_session_runtime_cannot_be_admitted_as_a_fresh_candidate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let prepared = revision(&project, Arc::clone(&table));
    let mut predecessor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();

    let mut applied = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    applied.apply(SessionPlaybackCommand::Pause).unwrap();
    assert_eq!(
        applied.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut rendered = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    rendered.render(&mut []).unwrap();
    assert_eq!(
        rendered.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut page_rendered = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    page_rendered
        .render_with_pages(&mut [], &mut ContinuityMissingPages)
        .unwrap();
    assert_eq!(
        page_rendered.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut seeded = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    seeded.seed_legacy_transport(0, 0, false).unwrap();
    assert_eq!(
        seeded.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
}

#[test]
fn accepted_session_candidate_refuses_second_handoff_before_either_runtime_changes() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let prepared = revision(&project, Arc::clone(&table));
    let clip = prepared.session().midi_clips()[0].id;

    let mut first_predecessor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    first_predecessor
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first_predecessor
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut prefix = [[0.0; 2]; 64];
    first_predecessor.render(&mut prefix).unwrap();
    let mut accepted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    accepted
        .try_inherit_state_from(&mut first_predecessor)
        .unwrap();

    let mut previous = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut previous_twin = SessionPlayback::new(prepared).unwrap();
    for playback in [&mut previous, &mut previous_twin] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut prefix).unwrap();
    }

    let accepted_frame = accepted.performance_frame;
    let accepted_routes = accepted.routes.clone();
    let accepted_release_signature: Vec<_> = accepted
        .releases
        .iter()
        .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
        .collect();
    let accepted_generation = accepted.route_state_generation;
    let previous_frame = previous.performance_frame;
    let previous_arrangement_frame = previous.arrangement_frame;
    let previous_routes = previous.routes.clone();
    let previous_release_signature: Vec<_> = previous
        .releases
        .iter()
        .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
        .collect();
    let previous_generation = previous.route_state_generation;
    assert_eq!(
        accepted.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
    assert_eq!(accepted.performance_frame, accepted_frame);
    assert_eq!(accepted.routes, accepted_routes);
    assert_eq!(
        accepted
            .releases
            .iter()
            .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
            .collect::<Vec<_>>(),
        accepted_release_signature
    );
    assert_eq!(accepted.route_state_generation, accepted_generation);
    assert_eq!(previous.performance_frame, previous_frame);
    assert_eq!(previous.arrangement_frame, previous_arrangement_frame);
    assert_eq!(previous.routes, previous_routes);
    assert_eq!(
        previous
            .releases
            .iter()
            .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
            .collect::<Vec<_>>(),
        previous_release_signature
    );
    assert_eq!(previous.route_state_generation, previous_generation);

    let mut previous_output = [[0.0; 2]; 128];
    let mut twin_output = [[0.0; 2]; 128];
    previous.render(&mut previous_output).unwrap();
    previous_twin.render(&mut twin_output).unwrap();
    assert_eq!(previous_output, twin_output);
    // The previously accepted candidate is also an audible owner. Refusal of
    // another handoff must not damage its inherited DSP continuation.
    let mut accepted_output = [[0.0; 2]; 128];
    accepted.render(&mut accepted_output).unwrap();
    assert_eq!(accepted_output, twin_output);
    assert!(
        accepted_output
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
}

#[test]
fn release_bank_refusal_keeps_fresh_candidate_retryable_after_predecessor_drains() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let original_revision = revision(&original, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut uninterrupted = SessionPlayback::new(original_revision).unwrap();
    let track = previous.routes[0].id;
    let clip = previous.revision.session().midi_clips()[0].id;
    let mut prefix = [[0.0; 2]; 64];
    for playback in [&mut previous, &mut uninterrupted] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut prefix).unwrap();
    }

    let removed_clip_revision = revision(&without_session_cells(&original), table);
    let mut candidate = SessionPlayback::new(removed_clip_revision).unwrap();
    candidate.release_capacity = 0;
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert!(candidate.candidate_admission_open);

    let mut after_refusal = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    previous.render(&mut after_refusal).unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(
        after_refusal, expected,
        "the donor remains audible after refusal"
    );
    assert!(after_refusal.iter().flatten().any(|sample| *sample != 0.0));
    let mut drain = vec![[0.0; 2]; 200_000 - 256];
    previous.render(&mut drain).unwrap();
    assert_eq!(previous.release_voice_count(), 0);
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(!candidate.candidate_admission_open);
    assert_eq!(candidate.performance_frame(), previous.performance_frame());
    assert_eq!(candidate.owner(track), Some(SessionPlaybackOwner::Silence));
}

#[test]
fn mutable_checkpoint_restores_held_and_released_feedback_with_owned_occurrence_ids() {
    for (duration, prefix_frames, expect_released) in
        [(8 << 32, 19, false), ((1 << 32) / 6_000, 5, true)]
    {
        let (revision, track) = mutable_checkpoint_fixture(duration);
        let mut source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut restored = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        for playback in [&mut source, &mut restored] {
            playback.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let prefix = render_checkpoint_prefix(&mut source, prefix_frames);
        let restored_prefix = render_checkpoint_prefix(&mut restored, prefix_frames);
        assert_eq!(prefix, restored_prefix);
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let source_runtime = source.mutable_instruments[0].as_ref().unwrap();
        let next_id = source_runtime.next_occurrence_id();
        assert_eq!(next_id, 1);
        let proof = source_runtime.occurrence_proofs().first().unwrap();
        assert_eq!(proof.id, next_id);
        assert_eq!(proof.note_off_sent, expect_released);
        assert!(source_runtime.committed_feedback_tail_is_active());

        let mut checkpoints = MutableRuntimeCheckpointStore::new();
        checkpoints.capture_track(&source, track).unwrap();
        assert_eq!(checkpoints.checkpoint_count(), 1);
        assert!(checkpoints.declared_storage_bytes() > 0);
        assert!(checkpoints.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);

        // Make the target's device state different while retaining its exact
        // revision and musical frame. Restore must recover both the voice
        // history and the engine's occurrence-ID ledger.
        reset_checkpoint_track(&mut restored, track);
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            0
        );
        checkpoints.restore_track(&mut restored, track).unwrap();
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            next_id
        );
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .occurrence_proofs(),
            source_runtime.occurrence_proofs()
        );
        checkpoints.restore_track(&mut source, track).unwrap();
        assert_eq!(
            source.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            next_id
        );

        let expected = render_checkpoint_partitioned(&mut source, &[1, 7, 3, 11, 2], 48);
        let actual = render_checkpoint_partitioned(&mut restored, &[5, 1, 9, 2, 7], 48);
        assert_eq!(actual, expected);
        // The same checkpoint remains reusable and the next occurrence still
        // receives the same ID after an identical continuation.
        let id_after = source.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id();
        assert_eq!(
            restored.mutable_instruments[0]
                .as_ref()
                .unwrap()
                .next_occurrence_id(),
            id_after
        );
    }
}

#[test]
fn arrangement_preroll_reconstructs_held_and_released_mutable_feedback() {
    let (prepared, _) = mutable_checkpoint_fixture(1 << 32);
    let untouched = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    caller.apply(SessionPlaybackCommand::Play).unwrap();
    let mut caller_prefix = [[0.0; 2]; 37];
    caller.render(&mut caller_prefix).unwrap();
    let caller_frame = caller.performance_frame();

    // A held note exercises both oscillator/voice continuation and feedback.
    // The later target is past the note gate and exercises the ended-gate tail.
    for target in [256_u64, 24_704] {
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();
        let mut remaining = target;
        let partitions = [19_usize, 64, 7, 127, 43];
        let mut partition_index = 0;
        while remaining != 0 {
            let count =
                remaining.min(partitions[partition_index % partitions.len()] as u64) as usize;
            let mut prefix = vec![[0.0; 2]; count];
            uninterrupted.render(&mut prefix).unwrap();
            remaining -= count as u64;
            partition_index += 1;
        }
        let mut expected = [[0.0; 2]; 193];
        uninterrupted.render(&mut expected).unwrap();

        let candidate = caller
            .prepare_arrangement_preroll(target, target, true, &AtomicBool::new(false))
            .unwrap();
        let mut candidate = Playback::from_arrangement_preroll(candidate).unwrap();
        assert!(Arc::ptr_eq(
            candidate.prepared_revision().expect("prepared revision"),
            &prepared
        ));
        assert_eq!(candidate.position(), target);
        assert_eq!(candidate.performance_frame(), target);
        assert!(candidate.playing());
        let mut actual = [[0.0; 2]; 193];
        candidate.render(&mut actual).unwrap();
        assert_eq!(actual, expected, "preroll continuation differs at {target}");
    }

    assert_eq!(caller.performance_frame(), caller_frame);
    assert!(caller.playing());
    assert_eq!(untouched.performance_frame(), 0);
    assert!(!untouched.playing());
}

#[test]
fn interactive_arrangement_preroll_accepts_session_routes_and_handoff() {
    interactive_preroll_session_command_is_available();
    interactive_preroll_warmed_state_inherits_into_compiled_revision();
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

#[test]
fn arrangement_seek_history_query_preserves_analytic_fast_path() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let sine = revision(&project(1.0, 0, 8 << 32), Arc::clone(&table));
    assert!(!sine.arrangement_seek_requires_preroll());

    let operator_state = crate::operator_legacy_test::legacy_operator_state();
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    snapshot.tracks[0].instrument.as_mut().unwrap().state = operator_state;
    let operator = revision(
        &Project::from_legacy_snapshot(snapshot).unwrap(),
        Arc::clone(&table),
    );
    assert!(!operator.arrangement_seek_requires_preroll());

    let compare_analytic_seek = |prepared: Arc<PreparedRevision>| {
        let destination = 24_704;
        let mut uninterrupted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        uninterrupted.apply(SessionPlaybackCommand::Play).unwrap();
        let _prefix = render_checkpoint_partitioned(
            &mut uninterrupted,
            &[17, 239, 61, 127],
            destination as usize,
        );
        let mut expected = [[0.0; 2]; 193];
        uninterrupted.render(&mut expected).unwrap();

        let mut seeked = SessionPlayback::new(prepared).unwrap();
        seeked
            .apply(SessionPlaybackCommand::Seek(destination))
            .unwrap();
        seeked.apply(SessionPlaybackCommand::Play).unwrap();
        let mut actual = [[0.0; 2]; 193];
        seeked.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
    };
    compare_analytic_seek(Arc::clone(&sine));
    compare_analytic_seek(Arc::clone(&operator));

    let (mutable, _) = mutable_checkpoint_fixture(8 << 32);
    assert!(mutable.arrangement_seek_requires_preroll());

    let filter_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut track_snapshot = project(1.0, 0, 8 << 32).snapshot();
    track_snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: track_snapshot.next_id,
        bypassed: false,
        state: filter_state.clone(),
    });
    track_snapshot.next_id += 1;
    let stateful_track = revision(
        &Project::from_legacy_snapshot(track_snapshot).unwrap(),
        Arc::clone(&table),
    );
    assert!(stateful_track.arrangement_seek_requires_preroll());

    let mut return_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let return_id = return_snapshot.next_id;
    let return_device_id = return_id + 1;
    return_snapshot.next_id = return_id + 2;
    return_snapshot.tracks[0]
        .sends
        .push(hex_project::SendLevelSnapshot {
            return_track: return_id,
            gain: 0.5,
        });
    return_snapshot
        .return_tracks
        .push(hex_project::ReturnTrackSnapshot {
            id: return_id,
            name: "return".into(),
            gain: 0.75,
            muted: false,
            devices: vec![DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: return_device_id,
                bypassed: false,
                state: filter_state,
            }],
        });
    let stateful_return = revision(
        &Project::from_legacy_snapshot(return_snapshot).unwrap(),
        table,
    );
    assert!(stateful_return.arrangement_seek_requires_preroll());
}

#[test]
fn operator_supported_state_edit_retargets_live_history_and_joins_later_notes() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    fn edited_state(
        feedback: f32,
        level: f32,
        carrier: f32,
    ) -> hex_lattice_contract::NativeDeviceState {
        let state = crate::operator_legacy_test::legacy_operator_state();
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::feedback(1),
                ParameterValue::Float(feedback),
            )
            .unwrap();
        let state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_level(1),
                ParameterValue::Float(level),
            )
            .unwrap();
        OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::carrier(1),
                ParameterValue::Float(carrier),
            )
            .unwrap()
    }

    fn run(
        partition: &[usize],
        session_route: bool,
        apply_intermediate: bool,
    ) -> Vec<crate::Frame> {
        let table = Arc::new(hex_devices::SineTable::new());
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        snapshot.tracks[0].instrument.as_mut().unwrap().state = edited_state(0.15, 0.45, 0.3);
        let arrangement = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        arrangement[0].notes.push(MidiNoteSnapshot {
            active: true,
            id: 7,
            start_raw: 1 << 24,
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 88,
        });
        let Some(session_cell) = snapshot.session.cells.first_mut() else {
            panic!("fixture has Session MIDI content");
        };
        let SessionCellContentSnapshot::Midi(session_clip) = &mut session_cell.content else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 1 << 24,
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 88,
        });
        snapshot.next_id += 1;
        let source_project = Project::from_legacy_snapshot(snapshot).unwrap();
        let old_revision = Arc::new(
            PreparedRevision::prepare(&source_project, &BTreeMap::new(), Arc::clone(&table), None)
                .unwrap(),
        );
        let track_id = old_revision.arrangement().tracks()[0]
            .canonical_track_id()
            .unwrap();
        assert!(
            old_revision
                .session()
                .mutable_instrument_factory(track_id)
                .is_some()
        );

        let instrument_id = source_project.tracks()[0].instrument().unwrap().id();
        let mut intermediate_editor = Editor::new(source_project.clone(), 4).unwrap();
        intermediate_editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state: edited_state(0.48, 0.7, 0.6),
            })
            .unwrap();
        let intermediate_project = intermediate_editor.project().clone();
        let intermediate_revision = Arc::new(
            PreparedRevision::prepare(
                &intermediate_project,
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(old_revision.session()),
            )
            .unwrap(),
        );
        assert!(
            intermediate_revision
                .session()
                .mutable_history_retarget(track_id)
                .is_some()
        );

        let mut final_editor = Editor::new(intermediate_project, 4).unwrap();
        final_editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state: edited_state(0.75, 0.9, 0.8),
            })
            .unwrap();
        // Deliberately prepare C from canonical B. The audio owner may still
        // have A, so C's target proof must be applicable to the actual runtime
        // rather than being chained to the most recently prepared revision.
        let candidate_revision = Arc::new(
            PreparedRevision::prepare(
                final_editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                Some(intermediate_revision.session()),
            )
            .unwrap(),
        );
        assert!(
            candidate_revision
                .session()
                .mutable_history_retarget(track_id)
                .is_some()
        );
        assert_eq!(
            candidate_revision
                .session()
                .mutable_history_retarget_storage_bytes(),
            112,
            "the inventory separately accounts the exact candidate proof"
        );

        let mut previous = SessionPlayback::new(old_revision).unwrap();
        if session_route {
            let clip = previous.revision.session().midi_clips()[0].id;
            previous
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-6));

        let mut successor = if apply_intermediate {
            let mut intermediate = SessionPlayback::new(intermediate_revision).unwrap();
            intermediate.try_inherit_state_from(&mut previous).unwrap();
            let mut ramp_prefix = [[0.0; 2]; 7];
            intermediate.render(&mut ramp_prefix).unwrap();
            SessionPlayback::new(candidate_revision)
                .and_then(|mut final_candidate| {
                    final_candidate.try_inherit_state_from(&mut intermediate)?;
                    Ok(final_candidate)
                })
                .unwrap()
        } else {
            let mut final_candidate = SessionPlayback::new(candidate_revision).unwrap();
            final_candidate
                .try_inherit_state_from(&mut previous)
                .unwrap();
            final_candidate
        };
        let mut suffix = Vec::new();
        let mut index = 0;
        while suffix.len() < 384 {
            let count = partition[index % partition.len()].min(384 - suffix.len());
            let mut block = vec![[0.0; 2]; count];
            successor.render(&mut block).unwrap();
            suffix.extend(block);
            index += 1;
        }
        assert!(suffix.iter().any(|frame| frame[0].abs() > 1.0e-6));
        suffix
    }

    for session_route in [false, true] {
        for apply_intermediate in [false, true] {
            let single_frame = run(&[1], session_route, apply_intermediate);
            assert_eq!(
                run(&[37, 128, 19, 64], session_route, apply_intermediate),
                single_frame
            );
        }
    }
}

#[test]
fn unsupported_operator_state_edit_waits_for_committed_quiescence() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    fn source_project(note_duration: u64, arrangement_note: bool) -> Project {
        let mut state = crate::operator_legacy_test::legacy_operator_state();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::feedback(1), ParameterValue::Float(0.25))
            .unwrap();
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        let instrument = snapshot.tracks[0].instrument.as_mut().unwrap();
        instrument.state = state;
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        if arrangement_note {
            clips[0].notes.push(MidiNoteSnapshot {
                active: true,
                id: snapshot.next_id,
                start_raw: 0,
                duration_raw: note_duration,
                pitch: 61,
                channel: 0,
                velocity: 100,
            });
            snapshot.next_id += 1;
        }
        let Some(session_cell) = snapshot.session.cells.first_mut() else {
            panic!("fixture has Session content");
        };
        let SessionCellContentSnapshot::Midi(session_clip) = &mut session_cell.content else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: note_duration,
            pitch: 61,
            channel: 0,
            velocity: 100,
        });
        snapshot.next_id += 1;
        Project::from_legacy_snapshot(snapshot).unwrap()
    }

    fn unsupported_candidate(
        source: &Project,
        instrument_id: hex_project::DeviceId,
        old_revision: &PreparedRevision,
        table: Arc<hex_devices::SineTable>,
    ) -> Arc<PreparedRevision> {
        let mut state = source.tracks()[0].instrument().unwrap().state().clone();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::waveform(1), ParameterValue::Choice(1))
            .unwrap();
        let mut editor = Editor::new(source.clone(), 4).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state,
            })
            .unwrap();
        let candidate = Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                table,
                Some(old_revision.session()),
            )
            .unwrap(),
        );
        let track = source.tracks()[0].id();
        assert!(
            !candidate
                .session()
                .mutable_history_retarget_requires_quiescence(track)
        );
        assert!(
            candidate
                .session()
                .mutable_history_retarget(track)
                .is_some()
        );
        candidate
    }

    let table = Arc::new(hex_devices::SineTable::new());

    // A held voice prevents a same-source unsupported law change from being
    // detached into an outgoing tail.
    let source = source_project(1 << 32, false);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate =
        unsupported_candidate(&source, instrument_id, &old_revision, Arc::clone(&table));
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    let session_clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut held_prefix = [[0.0; 2]; 128];
    previous.render(&mut held_prefix).unwrap();
    assert!(previous.routes.iter().enumerate().any(|(index, route)| {
        matches!(route.owner, RouteOwner::Session { .. })
            && previous
                .mutable_instruments
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|runtime| !runtime.committed_is_quiescent())
    }));
    let mut successor = SessionPlayback::new(candidate).unwrap();
    assert_eq!(
        successor.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert!(previous.playing());

    // A released voice still owns feedback/envelope tail state and receives
    // the same refusal until that committed tail expires.
    let source = source_project(1 << 30, true);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate =
        unsupported_candidate(&source, instrument_id, &old_revision, Arc::clone(&table));
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut released_prefix = vec![[0.0; 2]; 7_000];
    previous.render(&mut released_prefix).unwrap();
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut successor = SessionPlayback::new(candidate).unwrap();
    assert_eq!(
        successor.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );

    // The same edit is admissible after the prior runtime has no active ledger
    // entries or feedback tail; the new law starts from a fresh runtime.
    let source = source_project(1 << 32, true);
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );
    let candidate = unsupported_candidate(&source, instrument_id, &old_revision, table);
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    let mut successor = SessionPlayback::new(candidate).unwrap();
    successor.try_inherit_state_from(&mut previous).unwrap();
}

#[test]
fn skipped_unsupported_operator_edit_uses_actual_source_law() {
    use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let table = Arc::new(hex_devices::SineTable::new());
    let source = {
        let mut state = crate::operator_legacy_test::legacy_operator_state();
        state = OPERATOR_METADATA
            .patch_parameter(&state, parameter::feedback(1), ParameterValue::Float(0.25))
            .unwrap();
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        snapshot.tracks[0].instrument.as_mut().unwrap().state = state;
        let clips = match &mut snapshot.tracks[0].content {
            TrackContentSnapshot::Instrument(clips) => clips,
            TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
            _ => panic!("fixture has MIDI arrangement content"),
        };
        clips[0].notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: 1 << 32,
            pitch: 61,
            channel: 0,
            velocity: 100,
        });
        snapshot.next_id += 1;
        let SessionCellContentSnapshot::Midi(session_clip) = &mut snapshot.session.cells[0].content
        else {
            panic!("fixture has Session MIDI content");
        };
        session_clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: snapshot.next_id,
            start_raw: 0,
            duration_raw: 1 << 32,
            pitch: 61,
            channel: 0,
            velocity: 100,
        });
        snapshot.next_id += 1;
        Project::from_legacy_snapshot(snapshot).unwrap()
    };
    let instrument_id = source.tracks()[0].instrument().unwrap().id();
    let old_revision = Arc::new(
        PreparedRevision::prepare(&source, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
    );

    let edit = |base: &Project, state: hex_lattice_contract::NativeDeviceState| {
        let mut editor = Editor::new(base.clone(), 4).unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument_id,
                state,
            })
            .unwrap();
        editor.project().clone()
    };
    let unsupported_state = OPERATOR_METADATA
        .patch_parameter(
            &source.tracks()[0].instrument().unwrap().state().clone(),
            parameter::waveform(1),
            ParameterValue::Choice(1),
        )
        .unwrap();
    let skipped_project = edit(&source, unsupported_state);
    let skipped_revision = Arc::new(
        PreparedRevision::prepare(
            &skipped_project,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(old_revision.session()),
        )
        .unwrap(),
    );
    let compatible_state = OPERATOR_METADATA
        .patch_parameter(
            &source.tracks()[0].instrument().unwrap().state().clone(),
            parameter::feedback(1),
            ParameterValue::Float(0.75),
        )
        .unwrap();
    let final_project = edit(&skipped_project, compatible_state);
    let final_revision = Arc::new(
        PreparedRevision::prepare(
            &final_project,
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(skipped_revision.session()),
        )
        .unwrap(),
    );

    // B changes an immutable Operator law but is never published. C is
    // prepared from B, while the audio owner is still on A; target-bound proof
    // and actual-source validation should allow the compatible A -> C edit.
    let mut actual_a = SessionPlayback::new(old_revision.clone()).unwrap();
    let session_clip = actual_a.revision.session().midi_clips()[0].id;
    actual_a
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    actual_a.apply(SessionPlaybackCommand::Play).unwrap();
    let mut active = [[0.0; 2]; 128];
    actual_a.render(&mut active).unwrap();
    assert!(
        actual_a.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut direct_c = SessionPlayback::new(Arc::clone(&final_revision)).unwrap();
    direct_c.try_inherit_state_from(&mut actual_a).unwrap();

    // The same C target must refuse if B really was applied: its immutable
    // waveform law no longer matches the actual runtime while notes are live.
    let mut quiescent_a = SessionPlayback::new(old_revision).unwrap();
    let mut applied_b = SessionPlayback::new(skipped_revision).unwrap();
    applied_b.try_inherit_state_from(&mut quiescent_a).unwrap();
    let session_clip = applied_b.revision.session().midi_clips()[0].id;
    applied_b
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    applied_b.apply(SessionPlaybackCommand::Play).unwrap();
    let mut active = [[0.0; 2]; 128];
    applied_b.render(&mut active).unwrap();
    assert!(
        applied_b.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    let mut from_applied_b = SessionPlayback::new(final_revision).unwrap();
    assert_eq!(
        from_applied_b.try_inherit_state_from(&mut applied_b),
        Err(SessionPlaybackError::SourceHistoryUnavailable)
    );
    assert!(from_applied_b.candidate_admission_open);
}

fn mutable_feedback_with_stateful_track_and_return() -> Arc<PreparedRevision> {
    mutable_feedback_with_stateful_track_and_return_edited(|_| {})
}

fn mutable_feedback_with_stateful_track_and_return_edited(
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

#[test]
fn section_loop_preroll_starts_canonical_loop_with_empty_device_history() {
    thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let region = LoopRegion::new(24_000, 48_000, true).unwrap();
            let source = mutable_feedback_with_stateful_track_and_return();
            let oracle = mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
                snapshot.loop_region = Some(region)
            });
            assert!(source.arrangement().loop_region().is_none());
            // Section launches use the interactive Play-from law at the loop
            // start: no prefix replay, empty tails and chased held notes.
            let mut expected = SessionPlayback::new(oracle).unwrap();
            expected.play_from(24_000).unwrap();
            let mut history = SessionPlayback::new(Arc::clone(&expected.revision)).unwrap();
            history.apply(SessionPlaybackCommand::Play).unwrap();
            render_checkpoint_partitioned(&mut history, &[257], 24_000);
            let mut actual = SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&source),
                None,
                region,
                24_000,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(actual.loop_region(), Some(region));
            assert!(Arc::ptr_eq(&actual.revision, &source));
            assert_eq!(
                (actual.arrangement_frame(), actual.performance_frame()),
                (24_000, 24_000)
            );
            let mut reference = vec![[0.0; 2]; 48_129];
            let mut rendered = vec![[0.0; 2]; reference.len()];
            for chunk in reference.chunks_mut(257) {
                expected.render(chunk).unwrap();
            }
            for chunk in rendered.chunks_mut(127) {
                actual.render(chunk).unwrap();
            }
            assert_eq!(
                rendered, reference,
                "first pass, loop gates and empty tails"
            );
            let mut replayed = vec![[0.0; 2]; reference.len()];
            for chunk in replayed.chunks_mut(257) {
                history.render(chunk).unwrap();
            }
            assert_ne!(rendered, replayed, "prefix history is not reconstructed");
            assert_eq!(actual.arrangement_frame(), expected.arrangement_frame());
            assert_eq!(actual.performance_frame(), expected.performance_frame());
            assert!(
                source.arrangement().loop_region().is_none(),
                "saved loop remains unchanged"
            );
            let playback =
                crate::playback::Playback::from_interactive_arrangement_preroll(actual).unwrap();
            assert_eq!(playback.automation_capture_loop(), Some(region));
            assert_eq!(
                SessionPlayback::prepare_section_arrangement_preroll(
                    source,
                    None,
                    region,
                    24_000,
                    &AtomicBool::new(true),
                )
                .unwrap_err(),
                ArrangementPrerollError::Cancelled
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn section_loop_readmits_rounded_timing_without_rebuilding_note_resources() {
    thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| {
        let source = project_at_tempo(0.7, 0, 1 << 32, Tempo::new(123_000_000).unwrap());
        let region = LoopRegion::new(0, 23_415, true).unwrap();
        let prepared = revision(&source, Arc::new(hex_devices::SineTable::new()));
        let periodic = PreparedPeriodicSessionTempo::prepare(prepared.session().timing(), region).unwrap();
        assert_eq!(periodic.view(prepared.session().timing()).max_micro_bpm(), 123_000_001);
        let quote = SessionPlayback::section_runtime_allowance(&prepared, region).unwrap();
        let mut actual = SessionPlayback::prepare_section_arrangement_preroll(
            Arc::clone(&prepared), None, region, 0, &AtomicBool::new(false)).unwrap();
        assert!(actual.midi_admission.is_some(), "one micro-bpm rounding needs a new proof");
        assert!(actual.accounted_runtime_storage_bytes().unwrap() <= quote);
        let original = &prepared.session().midi_clips()[0];
        let effective = crate::session_inventory::EffectiveSessionMidi::new(prepared.session(), actual.midi_admission.as_ref()).midi_clip(original.id).unwrap();
        assert!(Arc::ptr_eq(original.instrument_template().note_bank(), effective.instrument_template().note_bank()), "note resources stay shared");
        assert_eq!(actual.session_work_envelope(), prepared.session().work());
        let mut snapshot = source.snapshot();
        snapshot.loop_region = Some(region);
        let oracle_source = Project::from_legacy_snapshot(snapshot).unwrap();
        let mut expected = SessionPlayback::new(revision(&oracle_source, Arc::new(hex_devices::SineTable::new()))).unwrap();
        for runtime in [&mut actual, &mut expected] {
            runtime.apply(SessionPlaybackCommand::LaunchClip { clip: original.id, quantization: LaunchQuantization::Immediate }).unwrap();
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut rendered = vec![[0.0; 2]; 48_013];
        let mut oracle = rendered.clone();
        for chunk in rendered.chunks_mut(127) { actual.render(chunk).unwrap(); }
        for chunk in oracle.chunks_mut(257) { expected.render(chunk).unwrap(); }
        assert_eq!(rendered, oracle, "effective source, interval index and release timing match a freshly prepared canonical loop");
        assert!(prepared.arrangement().loop_region().is_none());
        let playback = Playback::from_interactive_arrangement_preroll(actual).unwrap();
        let observation = playback.prepare_observation_on_control().unwrap();
        assert!(observation.retained_bytes_on_control().unwrap() <= playback.observation_storage_quote_on_control().unwrap());
        // Re-admission still refuses a truly dense periodic source. One note
        // per quarter is valid at123bpm but exceeds256 edges/256frames here.
        let mut snapshot = source.snapshot();
        let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else { panic!("midi fixture") };
        clip.source.source_length_raw = 1 << 32;
        clip.source.loop_end_raw = 1 << 32;
        clip.source.looping = true;
        let dense_source = Project::from_legacy_snapshot(snapshot).unwrap();
        let dense = revision(&dense_source, Arc::new(hex_devices::SineTable::new()));
        assert!(matches!(dense.session().midi_clips()[0].source_template().reprepare_for_timing_on_control(2_000_000_000_000), Err(crate::MidiTemplateError::TooManyEdges | crate::MidiTemplateError::TooManyOnsets)));
    }).unwrap().join().unwrap();
}

#[test]
fn arrangement_preroll_held_mutable_source_and_return_send_follow_seek_law() {
    let prepared = mutable_feedback_with_stateful_track_and_return();
    assert!(prepared.arrangement_seek_requires_preroll());
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    assert_eq!(caller.device_chains[0].stateful_count(), 1);
    assert_eq!(caller.return_chains[0].stateful_count(), 1);
    drop(caller);
    run_preroll_continuation_on_worker_stack(prepared, 512);
}

#[test]
fn interactive_arrangement_preroll_released_mutable_source_and_return_start_empty() {
    let prepared = mutable_feedback_with_stateful_track_and_return();
    assert!(prepared.arrangement_seek_requires_preroll());
    run_preroll_continuation_on_worker_stack(prepared, 24_704);
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

#[test]
fn interactive_arrangement_preroll_loop_destinations_follow_play_from_law() {
    thread::Builder::new()
        .name("hex-engine-loop-preroll-test".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let tempo = |snapshot: &mut Snapshot| {
                snapshot.tempo_points.push(TempoPoint::new(
                    BeatTime::from_ratio(1, 100).unwrap(),
                    Tempo::new(90_000_000).unwrap(),
                ));
                // This later authored transition is outside the loop. It must
                // not leak into the periodic performance clock after the seek.
                snapshot.tempo_points.push(TempoPoint::new(
                    BeatTime::from_ratio(1, 10).unwrap(),
                    Tempo::new(180_000_000).unwrap(),
                ));
            };
            let prepared = mutable_feedback_with_stateful_track_and_return_edited(|snapshot| {
                tempo(snapshot);
                snapshot.loop_region = Some(LoopRegion::new(128, 1024, true).unwrap());
            });
            for (requested, destination) in [
                (0, 0),
                (64, 64),
                (128, 128),
                (512, 512),
                (1024, 1024),
                (1408, 1408),
                (91_008, 91_008),
            ] {
                for playing in [false, true] {
                    assert_loop_preroll_continuation(&prepared, requested, destination, playing);
                }
            }
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

#[test]
fn interactive_arrangement_preroll_accepts_loops_and_checks_cancellation() {
    let mut looped_snapshot = project(0.5, 0, 1 << 32).snapshot();
    looped_snapshot.loop_region = Some(LoopRegion::new(0, 1024, true).unwrap());
    let looped_project = Project::from_legacy_snapshot(looped_snapshot).unwrap();
    let looped = revision(&looped_project, Arc::new(hex_devices::SineTable::new()));
    const SEEK_STACK_BYTES: usize = 8 * 1024 * 1024;
    thread::Builder::new()
        .name("hex-engine-interactive-refusal-test".into())
        .stack_size(SEEK_STACK_BYTES)
        .spawn(move || {
            let mut candidate =
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    Arc::clone(&looped),
                    128,
                    128,
                    true,
                    &AtomicBool::new(false),
                )
                .expect("enabled loop permits interactive seek");
            assert!(Arc::ptr_eq(&candidate.revision, &looped));
            assert_eq!(candidate.arrangement_frame(), 128);
            let mut actual = [[0.0; 2]; 1200];
            candidate.render(&mut actual).unwrap();
            assert_eq!(candidate.arrangement_frame(), 304);
            assert!(actual.iter().any(|frame| *frame != [0.0; 2]));
            assert_eq!(
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    Arc::clone(&looped),
                    128,
                    128,
                    false,
                    &AtomicBool::new(true),
                )
                .unwrap_err(),
                ArrangementPrerollError::Cancelled
            );
            // No prefix replays, so a post-loop destination needs no replay
            // work budget and keeps its absolute cursor.
            let post_loop = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                looped,
                1408,
                0,
                true,
                &AtomicBool::new(false),
            )
            .expect("interactive seek renders no prefix");
            assert_eq!(post_loop.arrangement_frame(), 1408);
        })
        .unwrap()
        .join()
        .unwrap();
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

#[test]
fn interactive_and_section_destinations_exceed_the_linear_replay_ceiling() {
    // 120 bpm at 48 kHz: beat 1398 starts at frame 33_552_000, so the
    // destination lands inside the clip's first held note.
    let destination = MAX_ARRANGEMENT_PREROLL_FRAMES * 4 + 37;
    let mut snapshot = project(0.5, 0, 1 << 32).snapshot();
    let (TrackContentSnapshot::Instrument(clips)
    | TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    }) = &mut snapshot.tracks[0].content
    else {
        panic!("fixture track holds midi clips");
    };
    clips[0].start_raw = 1398 << 32;
    let prepared = revision(
        &Project::from_legacy_snapshot(snapshot).unwrap(),
        Arc::new(hex_devices::SineTable::new()),
    );
    // Linear export still replays the prefix and keeps its work ceiling.
    assert_eq!(
        SessionPlayback::prepare_arrangement_preroll_from_revision(
            Arc::clone(&prepared),
            destination,
            MAX_ARRANGEMENT_PREROLL_FRAMES,
            true,
            &AtomicBool::new(false),
        )
        .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );
    thread::Builder::new()
        .name("hex-engine-long-seek-test".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut expected = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
            expected.play_from(destination).unwrap();
            let mut expected_audio = [[0.0; 2]; 129];
            expected.render(&mut expected_audio).unwrap();
            assert!(expected_audio.iter().any(|frame| *frame != [0.0; 2]));
            for start_playing in [true, false] {
                let mut candidate =
                    SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                        Arc::clone(&prepared),
                        destination,
                        destination,
                        start_playing,
                        &AtomicBool::new(false),
                    )
                    .expect("interactive seek beyond the replay ceiling");
                assert_eq!(candidate.arrangement_frame(), destination);
                assert_eq!(candidate.playing(), start_playing);
                if !start_playing {
                    candidate.apply(SessionPlaybackCommand::Play).unwrap();
                }
                let mut actual = [[0.0; 2]; 129];
                candidate.render(&mut actual).unwrap();
                assert_eq!(
                    actual, expected_audio,
                    "held note is chased at {destination}"
                );
            }
            let region = LoopRegion::new(destination, destination + 4096, true).unwrap();
            let section = SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&prepared),
                None,
                region,
                destination,
                &AtomicBool::new(false),
            )
            .expect("section launch beyond the replay ceiling");
            assert_eq!(section.arrangement_frame(), destination);
            assert!(section.playing());
            // Play-from arithmetic still bounds the destination.
            assert_eq!(
                SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    prepared,
                    u64::MAX,
                    u64::MAX,
                    true,
                    &AtomicBool::new(false),
                )
                .unwrap_err(),
                ArrangementPrerollError::Playback(SessionPlaybackError::ArithmeticOverflow)
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn arrangement_preroll_checks_work_cancellation_loop_and_pause_contracts() {
    let prepared = revision(
        &project(0.5, 0, 1 << 32),
        Arc::new(hex_devices::SineTable::new()),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        caller
            .prepare_arrangement_preroll(256, 256, false, &cancelled)
            .unwrap_err(),
        ArrangementPrerollError::Cancelled
    );
    assert_eq!(
        caller
            .prepare_arrangement_preroll(257, 256, false, &AtomicBool::new(false))
            .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );
    assert_eq!(
        caller
            .prepare_arrangement_preroll(
                u64::MAX,
                MAX_ARRANGEMENT_PREROLL_FRAMES,
                false,
                &AtomicBool::new(false),
            )
            .unwrap_err(),
        ArrangementPrerollError::WorkBudgetExceeded
    );

    let paused = caller
        .prepare_arrangement_preroll(256, 256, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(paused.performance_frame(), 256);
    assert_eq!(paused.arrangement_frame(), 256);
    assert!(!paused.playing());

    let polls = AtomicUsize::new(0);
    let error = caller
        .prepare_arrangement_preroll_checked(
            512,
            512,
            true,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || polls.fetch_add(1, Ordering::Relaxed) >= 3,
        )
        .unwrap_err();
    assert_eq!(error, ArrangementPrerollError::Cancelled);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());

    let final_block_polls = AtomicUsize::new(0);
    let error = caller
        .prepare_arrangement_preroll_checked(
            256,
            256,
            true,
            MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            || final_block_polls.fetch_add(1, Ordering::Relaxed) >= 3,
        )
        .unwrap_err();
    assert_eq!(error, ArrangementPrerollError::Cancelled);
    assert_eq!(final_block_polls.load(Ordering::Relaxed), 4);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
}

#[test]
fn arrangement_preroll_accepts_zero_frame_and_refuses_transport_loop() {
    let prepared = revision(
        &project(0.5, 0, 1 << 32),
        Arc::new(hex_devices::SineTable::new()),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let zero = caller
        .prepare_arrangement_preroll(0, 0, false, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(zero.performance_frame(), 0);
    assert_eq!(zero.arrangement_frame(), 0);
    assert!(!zero.playing());
    let zero_playing = caller
        .prepare_arrangement_preroll(0, 0, true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(zero_playing.performance_frame(), 0);
    assert_eq!(zero_playing.arrangement_frame(), 0);
    assert!(zero_playing.playing());

    let mut looped_snapshot = project(0.5, 0, 1 << 32).snapshot();
    looped_snapshot.loop_region = Some(LoopRegion::new(0, 1024, true).unwrap());
    let looped = Project::from_legacy_snapshot(looped_snapshot).unwrap();
    let looped = revision(&looped, Arc::new(hex_devices::SineTable::new()));
    let looped_caller = SessionPlayback::new(Arc::clone(&looped)).unwrap();
    assert_eq!(
        looped_caller
            .prepare_arrangement_preroll(128, 128, false, &AtomicBool::new(false))
            .unwrap_err(),
        ArrangementPrerollError::UnsupportedTransportLoop
    );
}

#[test]
fn arrangement_preroll_empty_project_can_advance_beyond_content_end() {
    let empty_project = Project::new(48_000).unwrap();
    let empty_revision = revision(&empty_project, Arc::new(hex_devices::SineTable::new()));
    let empty_caller = SessionPlayback::new(Arc::clone(&empty_revision)).unwrap();
    let mut candidate = empty_caller
        .prepare_arrangement_preroll(12_345, 12_345, true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(candidate.performance_frame(), 12_345);
    assert_eq!(candidate.arrangement_frame(), 12_345);
    assert!(candidate.playing());
    let mut silence = [[1.0, -1.0]; 16];
    candidate.render(&mut silence).unwrap();
    assert!(silence.iter().all(|frame| *frame == [0.0, 0.0]));
    assert_eq!(empty_caller.performance_frame(), 0);
    assert!(!empty_caller.playing());
}

#[test]
fn arrangement_preroll_refuses_storage_preflight_before_runtime_factory() {
    let (prepared, _) = mutable_checkpoint_fixture(8 << 32);
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| {
        bytes.set(Some(hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES));
    });
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert!(preflight > 1);
    let result =
        caller.prepare_arrangement_preroll_checked(256, 256, true, preflight - 1, || false);
    assert_eq!(
        result.unwrap_err(),
        ArrangementPrerollError::StorageBudgetExceeded
    );
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn linear_audio_only_export_omits_unused_banks_preserves_pcm_and_refuses_console() {
    let source: Vec<Frame> = (0..32)
        .map(|i| [i as f32 / 64.0, -(i as f32) / 128.0])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    snapshot.next_id = 8;
    snapshot.tracks[0].content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
        active: true,
        name: "arrangement".into(),
        flavor: hex_project::ClipFlavor::default(),
        edit: None,
        gain: hex_project::ClipGain::UNITY,
        id: 7,
        asset: 4,
        start: 0,
        duration: 32,
        source_offset: 0,
        source_length: 32,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(32).unwrap(),
    }]);
    let project = Project::from_snapshot(snapshot).unwrap();
    // Unlinked authoring retains its MIDI metadata wrapper, with no compiled
    // note spans or generator. It must not spend interactive release storage.
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Track(unbound) = editor
        .execute(Command::AddMidiTrack {
            name: "unbound".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: unbound,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    let media = audio_media(project, &audio, &audio);
    let revision = Arc::new(
        PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
            .unwrap(),
    );
    let interactive_quote =
        super::preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    assert_eq!(
        SessionPlayback::interactive_runtime_allowance(&revision).unwrap(),
        interactive_quote
    );
    let quote = SessionPlayback::linear_export_runtime_allowance(&revision).unwrap();
    let omitted = 2
        * SESSION_RELEASE_CAPACITY
        * (std::mem::size_of::<ReleaseEntry>() + std::mem::size_of::<MidiOccurrenceSuppression>());
    assert_eq!(interactive_quote - quote, omitted);
    let mut interactive = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert_eq!(
        interactive.release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
    let mut export = SessionPlayback::new_linear_arrangement_export(revision).unwrap();
    assert_eq!(export.release_storage_capacity(), 0);
    assert_eq!(export.releases.capacity(), 0);
    assert_eq!(export.release_scratch.capacity(), 0);
    assert_eq!(export.suppressions.capacity(), 0);
    assert_eq!(export.suppression_scratch.capacity(), 0);
    assert_eq!(
        export.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES,
        quote
    );
    assert!(export.try_inherit_state_from(&mut interactive).is_err());
    assert!(
        export
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: export.revision.session().audio_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .is_err()
    );
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 40];
    export.render(&mut output).unwrap();
    assert_eq!(output[..32], source);
    assert_eq!(output[32..], [[0.0; 2]; 8]);

    let prepare = |budget| {
        SessionPlayback::prepare_arrangement_preroll_from_revision_checked(
            Arc::clone(&interactive.revision),
            4,
            4,
            true,
            budget,
            || false,
        )
    };
    assert_eq!(
        prepare(quote - 1).unwrap_err(),
        ArrangementPrerollError::StorageBudgetExceeded
    );
    let mut replay = prepare(quote).unwrap();
    let mut next = [[0.0; 2]; 4];
    replay.render(&mut next).unwrap();
    assert_eq!(next, source[4..8]);

    let (midi, _) = mutable_checkpoint_fixture(8 << 32);
    let midi_quote = super::preroll::arrangement_runtime_storage_preflight_bytes(&midi).unwrap();
    assert_eq!(
        SessionPlayback::linear_export_runtime_allowance(&midi).unwrap(),
        midi_quote
    );
    let midi_export = SessionPlayback::new_linear_arrangement_export(midi).unwrap();
    assert_eq!(
        midi_export.release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
}

#[test]
fn linear_receiverless_bound_source_uses_audio_only_quote_and_renders_silence() {
    let mut editor = Editor::new(project(1.0, 0, 1 << 32), 8).unwrap();
    let receiver = editor.project().tracks()[0].id();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(sender) = editor
        .execute(Command::AddMidiTrack {
            name: "sender".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: sender,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    assert_eq!(editor.project().source_contributors(source).count(), 2);
    assert!(
        !editor.project().track(receiver).unwrap().midi_clips()[0]
            .notes()
            .is_empty()
    );
    let prepared = revision(editor.project(), Arc::new(crate::SineTable::new()));
    assert!(
        prepared
            .arrangement()
            .tracks()
            .iter()
            .all(|track| !track.receives_source_audio())
    );
    let quote = SessionPlayback::linear_export_runtime_allowance(&prepared).unwrap();
    let mut export = SessionPlayback::new_linear_arrangement_export(prepared).unwrap();
    assert_eq!(export.release_storage_capacity(), 0);
    assert!(
        export.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES
            <= quote
    );
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[1.0; 2]; 64];
    export.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 64]);

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(receiver),
        })
        .unwrap();
    let prepared = revision(editor.project(), Arc::new(crate::SineTable::new()));
    let full = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert_eq!(
        SessionPlayback::linear_export_runtime_allowance(&prepared).unwrap(),
        full
    );
    assert_eq!(
        SessionPlayback::new_linear_arrangement_export(prepared)
            .unwrap()
            .release_storage_capacity(),
        SESSION_RELEASE_CAPACITY
    );
}

#[test]
fn resident_batch_storage_is_included_in_preroll_admission() {
    let fixture = crate::signal_graph::tests::fixture(&[&[7], &[]], 32);
    let revision = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let runtime = SessionPlayback::new(revision).unwrap();
    assert_eq!(runtime.resident_source_scratch.len(), 2);
    let actual =
        runtime.accounted_runtime_storage_bytes().unwrap() + super::preroll::PREROLL_SCRATCH_BYTES;
    assert!(
        actual <= preflight,
        "actual={actual}, preflight={preflight}"
    );
}

#[test]
fn arrangement_preroll_storage_preflight_covers_constructed_runtime() {
    let (prepared, _) = mutable_checkpoint_fixture(8 << 32);
    let preflight = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let candidate = caller
        .prepare_arrangement_preroll(1_024, 1_024, false, &AtomicBool::new(false))
        .unwrap();
    let actual = candidate.accounted_runtime_storage_bytes().unwrap();
    let actual_with_scratch = actual + super::preroll::PREROLL_SCRATCH_BYTES;
    assert!(
        actual_with_scratch <= preflight,
        "actual_with_scratch={actual_with_scratch}, preflight={preflight}"
    );
    assert!(preflight <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    eprintln!("preroll runtime storage: preflight={preflight} actual={actual_with_scratch}");
}

#[test]
fn arrangement_preroll_returns_no_candidate_when_source_render_fails() {
    let table = Arc::new(hex_devices::SineTable::new());
    let source_project = session_audio_midi_arrangement_mix_project();
    let asset = source_project
        .assets()
        .find(|asset| asset.id().value() == 4)
        .unwrap();
    let streamed = StreamedSourceMetadata::new(
        StreamSourceKey::new(82, asset.id()),
        asset.source_sample_rate(),
        source_project.sample_rate(),
        asset.source_frames(),
        asset.playback_frames(),
        asset.conversion_version(),
        [0x82; 32],
    )
    .unwrap();
    let prepared_audio = source_project
        .assets()
        .map(|asset| {
            let source = if asset.id().value() == 4 {
                PreparedAudioSource::Streamed(Arc::new(streamed.clone()))
            } else {
                PreparedAudioSource::Resident(
                    crate::Audio::new(48_000, vec![[0.25, -0.25]; 32]).unwrap(),
                )
            };
            (asset.id(), source)
        })
        .collect::<BTreeMap<_, _>>();
    let prepared = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &source_project,
            &prepared_audio,
            82,
            table,
            None,
        )
        .unwrap(),
    );
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let error = caller
        .prepare_arrangement_preroll(256, 256, true, &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(
        error,
        ArrangementPrerollError::Playback(SessionPlaybackError::MediaPagesUnavailable)
    );
    assert_eq!(caller.performance_frame(), 0);
    assert!(!caller.playing());
}

#[test]
#[ignore = "release-only timing evidence; run with --release and --ignored"]
fn arrangement_preroll_profiles_100_stateful_tracks() {
    let prepared = mutable_hundred_track_preroll_revision();
    let caller = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let target = 16_384;
    let storage = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    assert_eq!(caller.route_count(), 100);
    let mut elapsed = Vec::new();
    for _ in 0..3 {
        let start = std::time::Instant::now();
        let candidate = caller
            .prepare_arrangement_preroll(target, target, false, &AtomicBool::new(false))
            .unwrap();
        elapsed.push(start.elapsed());
        assert_eq!(candidate.performance_frame(), target);
        assert!(!candidate.playing());
    }
    eprintln!(
        "100-track mutable preroll: sample_rate=48000 target_frames={target} blocks={} storage_upper_bound={storage} elapsed={elapsed:?}",
        target / 256
    );
}

#[test]
#[ignore = "release-only timing evidence; run with --release and --ignored"]
fn interactive_arrangement_preroll_profiles_100_stateful_tracks() {
    let prepared = mutable_hundred_track_preroll_revision();
    let target = 48_000;
    let storage = super::preroll::arrangement_runtime_storage_preflight_bytes(&prepared).unwrap();
    let timings = std::thread::Builder::new()
        .name("interactive-preroll-profile".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut elapsed = Vec::with_capacity(3);
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let candidate =
                    SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                        Arc::clone(&prepared),
                        target,
                        target,
                        false,
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                elapsed.push(start.elapsed());
                assert_eq!(candidate.performance_frame(), target);
                assert!(!candidate.playing());
            }
            elapsed
        })
        .unwrap()
        .join()
        .unwrap();
    eprintln!(
        "100-track interactive mutable preroll: sample_rate=48000 target_frames={target} blocks={} storage_upper_bound={storage} session_playback_bytes={} elapsed={timings:?}; excludes native callback/publication and is not a readiness deadline",
        target.div_ceil(256),
        std::mem::size_of::<SessionPlayback>()
    );
}

#[test]
fn mutable_checkpoint_refuses_uncommitted_wrong_revision_and_wrong_frame_atomically() {
    let (revision, track) = mutable_checkpoint_fixture(8 << 32);
    let mut source = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut target = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    for playback in [&mut source, &mut target] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(playback, 16);
    }
    let mut checkpoints = MutableRuntimeCheckpointStore::new();
    checkpoints.capture_track(&source, track).unwrap();

    let wrong_frame_before = target.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .next_occurrence_id();
    render_checkpoint_prefix(&mut target, 1);
    assert_eq!(
        checkpoints.restore_track(&mut target, track),
        Err(MutableRuntimeCheckpointError::IncompatiblePerformanceFrame)
    );
    assert_eq!(
        target.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        wrong_frame_before
    );

    let instrument_id =
        hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: instrument_id,
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let changed_project = with_test_instrument(&project(1.0, 0, 8 << 32), instrument_id, 1);
    let changed_revision = revision_with_instrument_registry(
        &changed_project,
        Arc::new(hex_devices::SineTable::new()),
        &registry,
    );
    let mut wrong_revision = SessionPlayback::new(changed_revision).unwrap();
    wrong_revision.apply(SessionPlaybackCommand::Play).unwrap();
    render_checkpoint_prefix(&mut wrong_revision, 16);
    let wrong_revision_id = wrong_revision.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .next_occurrence_id();
    assert_eq!(
        checkpoints.restore_track(&mut wrong_revision, track),
        Err(MutableRuntimeCheckpointError::IncompatiblePreparedRevision)
    );
    assert_eq!(
        wrong_revision.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .next_occurrence_id(),
        wrong_revision_id
    );

    source.mutable_instruments[0]
        .as_mut()
        .unwrap()
        .begin_uncommitted_test_block();
    let mut empty = MutableRuntimeCheckpointStore::new();
    let empty_store_bytes = empty.declared_storage_bytes();
    assert_eq!(
        empty.capture_track(&source, track),
        Err(MutableRuntimeCheckpointError::UncommittedRuntime)
    );
    assert_eq!(empty.checkpoint_count(), 0);
    assert_eq!(empty.declared_storage_bytes(), empty_store_bytes);
    assert!(empty_store_bytes > 0);
}

#[test]
fn mutable_checkpoint_store_bounds_factory_calls_before_construction() {
    let counter = Arc::new(AtomicUsize::new(0));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&counter)));
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| {
        bytes.set(Some(hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES));
    });
    let (revision, track) = mutable_checkpoint_fixture(8 << 32);
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    render_checkpoint_prefix(&mut playback, 1);
    let mut byte_limited = MutableRuntimeCheckpointStore::new();
    let byte_refusal = loop {
        match byte_limited.capture_track(&playback, track) {
            Ok(()) => {
                render_checkpoint_prefix(&mut playback, 1);
            }
            Err(error) => break error,
        }
    };
    assert_eq!(
        byte_refusal,
        MutableRuntimeCheckpointError::CheckpointBudgetExceeded
    );
    assert!(byte_limited.checkpoint_count() < MAX_MUTABLE_RUNTIME_CHECKPOINTS);
    assert!(byte_limited.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);
    let before_byte_refusal = counter.load(Ordering::Relaxed);
    assert_eq!(
        byte_limited.capture_track(&playback, track),
        Err(MutableRuntimeCheckpointError::CheckpointBudgetExceeded)
    );
    assert_eq!(counter.load(Ordering::Relaxed), before_byte_refusal);

    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    // Minimize processor charge with the truthful zero-tail factory. A source
    // checkpoint also reserves its mandatory live ledger, so the byte ceiling
    // may be reached before the independent entry-count ceiling.
    let (revision, track) = mutable_checkpoint_fixture_with_release(8 << 32, 0);
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut source_bounded = MutableRuntimeCheckpointStore::new();
    let byte_refusal_with_source_ledger = loop {
        render_checkpoint_prefix(&mut playback, 1);
        let before_attempt = counter.load(Ordering::Relaxed);
        match source_bounded.capture_track(&playback, track) {
            Ok(()) => {}
            Err(error) => {
                assert_eq!(
                    error,
                    MutableRuntimeCheckpointError::CheckpointBudgetExceeded
                );
                assert_eq!(counter.load(Ordering::Relaxed), before_attempt);
                break error;
            }
        }
    };
    assert!(source_bounded.checkpoint_count() < MAX_MUTABLE_RUNTIME_CHECKPOINTS);
    let before_refusal = counter.load(Ordering::Relaxed);
    render_checkpoint_prefix(&mut playback, 1);
    assert_eq!(
        source_bounded.capture_track(&playback, track),
        Err(byte_refusal_with_source_ledger)
    );
    assert!(source_bounded.declared_storage_bytes() <= MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES);
    assert_eq!(counter.load(Ordering::Relaxed), before_refusal);
    source_bounded.clear();
    assert_eq!(source_bounded.checkpoint_count(), 0);
    assert_eq!(
        source_bounded.declared_storage_bytes(),
        MutableRuntimeCheckpointStore::new().declared_storage_bytes()
    );
    MUTABLE_RUNTIME_DECLARED_BYTES_OVERRIDE.with(|bytes| bytes.set(None));
    MUTABLE_RUNTIME_CREATE_COUNTER.with(|slot| *slot.borrow_mut() = None);
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
#[test]
fn instrument_metering_preserves_pcm_and_host_trim_keeps_live_note_identity() {
    for input in [true, false] {
        let project = project(0.758_499_15, 0, 8 << 32);
        let table = Arc::new(hex_devices::SineTable::new());
        let prepared = revision(&project, Arc::clone(&table));
        let device = project.tracks()[0].instrument().unwrap().id();
        let meter = Arc::clone(prepared.arrangement().device_meter(device.value()).unwrap());
        let mut ordinary = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        let track = project.tracks()[0].id();
        let observation =
            crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
        let mut metered = SessionPlayback::new(prepared).unwrap();
        metered.set_device_metering(true);
        ordinary.apply(SessionPlaybackCommand::Play).unwrap();
        metered.apply(SessionPlaybackCommand::Play).unwrap();
        let mut a = [[0.0; 2]; 127];
        let mut b = a;
        let mut output_peak = 0.0_f32;
        for _ in 0..5 {
            ordinary.render(&mut a).unwrap();
            metered.render(&mut b).unwrap();
            assert_eq!(a, b, "telemetry must preserve per-voice summation order");
            output_peak = b
                .iter()
                .flatten()
                .fold(output_peak, |peak, sample| peak.max(sample.abs()));
        }
        let track_meter = observation.track_output_meter(track).unwrap();
        assert_eq!(track_meter.take_track_output(), output_peak);
        assert_eq!(track_meter.take_track_output(), 0.0);
        let peaks = meter.take_peaks();
        assert_eq!(peaks.input, 0.0);
        assert!(peaks.output.iter().any(|peak| *peak > 0.0));
        let mut snapshot = project.snapshot();
        let instrument = snapshot.tracks[0].instrument.as_mut().unwrap();
        instrument.state = if input {
            instrument.state.clone().with_input_gain_db(-12.0).unwrap()
        } else {
            instrument.state.clone().with_output_gain_db(-12.0).unwrap()
        };
        let changed = Project::from_legacy_snapshot(snapshot).unwrap();
        let prepared = Arc::new(
            PreparedRevision::prepare(
                &changed,
                &BTreeMap::new(),
                table,
                Some(ordinary.revision.session()),
            )
            .unwrap(),
        );
        let mut trimmed = SessionPlayback::new(prepared).unwrap();
        trimmed.try_inherit_state_from(&mut ordinary).unwrap();
        assert!(
            trimmed.releases.is_empty(),
            "host trim must not create a duplicate voice release"
        );
        ordinary.render(&mut a).unwrap();
        trimmed.render(&mut b).unwrap();
        let gain = if input {
            1.0
        } else {
            10.0_f32.powf(-12.0 / 20.0)
        };
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            assert!((a * gain - b).abs() < 1e-7);
        }
    }
}

#[test]
fn meter_capture_combines_each_owner_before_advancing_and_discards_refused_blocks() {
    use super::capture::{CaptureSink, OutputCaptureId};
    use super::meter_capture::{Cell, MeterCapture};

    let mut editor = Editor::new(project(1.0, 0, 8 << 32), 8).unwrap();
    let first = editor.project().tracks()[0].id();
    let Created::Track(second) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("fixture track");
    };
    let prepared = revision(editor.project(), Arc::new(hex_devices::SineTable::new()));
    let observation = crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    let mut cells = vec![
        Cell::new(OutputCaptureId::Track(first)),
        Cell::new(OutputCaptureId::Track(second)),
    ];
    super::meter_capture::sort(&mut cells);
    let mut capture = MeterCapture {
        revision: &prepared,
        cells: &mut cells,
        downstream: None,
        invalidated: false,
    };
    // Owners can be interleaved; two contributions to the first owner at the
    // same offset cancel before the peak is measured.
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.5, 0.0]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(second), 1, &[[0.9, 0.0]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 0, &[[-0.25, 0.0]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 1, &[[0.1, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output(),
        0.25
    );
    assert_eq!(
        observation
            .track_output_meter(second)
            .unwrap()
            .take_track_output(),
        0.9
    );

    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.3, 0.0]])
        .unwrap();
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.3, 0.0], [0.7, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output(),
        0.7
    );

    capture
        .add(OutputCaptureId::Track(first), 5, &[[0.8, 0.0]])
        .unwrap();
    // Out-of-order telemetry is discarded without interrupting audio.
    capture
        .add(OutputCaptureId::Track(first), 4, &[[0.1, 0.0]])
        .unwrap();
    capture.commit(2);
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output(),
        0.0
    );
    capture
        .add(OutputCaptureId::Track(first), 0, &[[0.7, 0.0]])
        .unwrap();
    capture.invalidate();
    assert_eq!(
        observation
            .track_output_meter(first)
            .unwrap()
            .take_track_output(),
        0.0
    );
}

#[test]
fn track_metering_preserves_multiple_track_audio() {
    let original = project(0.758_499_15, 0, 8 << 32);
    let mut snapshot = original.snapshot();
    let clips = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        _ => panic!("fixture must contain MIDI"),
    };
    {
        let mut note = clips[0].notes[0].clone();
        note.id = 24;
        note.pitch = 67;
        clips[0].notes.push(note);
    }
    let mut second = snapshot.tracks[0].clone();
    second.id = 20;
    second.gain = 0.431;
    second.instrument.as_mut().unwrap().id = 21;
    let clips = match &mut second.content {
        TrackContentSnapshot::Instrument(clips) => clips,
        TrackContentSnapshot::Hybrid { midi_clips, .. } => midi_clips,
        _ => panic!("fixture must contain MIDI"),
    };
    {
        clips[0].id = 22;
        clips[0].notes[0].id = 23;
        clips[0].notes[1].id = 25;
    }
    snapshot.next_id = 26;
    snapshot.tracks.push(second);
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
    let prepared = revision(&project, Arc::new(hex_devices::SineTable::new()));
    let observation = crate::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap();
    let mut ordinary = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut metered = SessionPlayback::new(prepared).unwrap();
    metered.set_device_metering(true);
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    metered.apply(SessionPlaybackCommand::Play).unwrap();
    let mut a = [[0.0; 2]; 127];
    let mut b = a;
    for _ in 0..5 {
        ordinary.render(&mut a).unwrap();
        metered.render(&mut b).unwrap();
        // The pre-existing instrument-meter path groups per-track sums, so
        // metered and unmetered renderers can differ by f32 rounding.
        for (actual, reference) in b.iter().flatten().zip(a.iter().flatten()) {
            assert!((actual - reference).abs() < 1e-7);
        }
    }
    for track in project.tracks() {
        assert!(
            observation
                .track_output_meter(track.id())
                .unwrap()
                .take_track_output()
                > 0.0
        );
    }
}

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

#[test]
fn closed_candidate_main_checks_prepared_and_actual_runtime_before_any_groups() {
    use hex_lattice_contract::NativeDeviceMetadata;
    fn runtime(with_main: bool) -> SessionPlayback {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        if with_main {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
        }
        let revision = PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap();
        SessionPlayback::new(Arc::new(revision)).unwrap()
    }
    let mut empty = runtime(false);
    let mut main = runtime(true);
    let mut locations = std::collections::BTreeSet::new();
    empty
        .visit_candidate_groups(&mut |kind, index, _| {
            assert!(locations.insert((kind, index)), "duplicate owning location");
            Ok(())
        })
        .unwrap();
    assert_eq!(
        locations.len() + 5,
        candidate_group_reservation(0, 0).unwrap()
    );
    // Both prepared and runtime Main are real populated chains, not placeholders.
    assert!(!main.revision.arrangement().main_device_chain.is_empty());
    assert!(main.main_chain.storage_view().entries().next().is_some());
    // Fresh matched Main storage is now supported; owning handoff remains
    // separately refused until its origin transfer is represented.
    main.visit_candidate_groups(&mut |_, _, _| Ok(())).unwrap();
    let refuses_without_emitting = |runtime: &SessionPlayback| {
        let mut emitted = 0;
        assert_eq!(
            runtime.visit_candidate_groups(&mut |_, _, _| {
                emitted += 1;
                Ok(())
            }),
            Err(crate::AudioStorageError::UnsupportedGraph)
        );
        assert_eq!(emitted, 0);
    };
    std::mem::swap(&mut empty.main_chain, &mut main.main_chain);
    refuses_without_emitting(&empty); // runtime-only population
    refuses_without_emitting(&main); // prepared-only population
}

mod cue_isolation;

#[test]
fn analytic_clip_volume_survives_track_stop_and_removed_track_handoffs() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    for session in [false, true] {
        for removed in [false, true] {
            let run = |volume: bool| {
                let mut editor = Editor::new(project(0.5, 0, 8 << 32), 8).unwrap();
                let track = editor.project().tracks()[0].id();
                let clip = if session {
                    revision(editor.project(), Arc::new(hex_devices::SineTable::new()))
                        .session()
                        .midi_clips()[0]
                        .id
                } else {
                    editor.project().track(track).unwrap().midi_clips()[0].id()
                };
                if volume {
                    let curve = MidiClipVolume::new(
                        vec![MidiClipVolumePoint::linear(
                            BeatTime::ZERO,
                            MidiClipGain::new(0.25).unwrap(),
                        )],
                        editor.project().midi_source(clip).unwrap().source_length(),
                    )
                    .unwrap();
                    editor
                        .execute(Command::ReplaceMidiClipVolume {
                            clip,
                            volume: curve,
                        })
                        .unwrap();
                }
                let table = Arc::new(hex_devices::SineTable::new());
                let mut playback =
                    SessionPlayback::new(revision(editor.project(), Arc::clone(&table))).unwrap();
                if session {
                    playback
                        .apply(SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                playback.apply(SessionPlaybackCommand::Play).unwrap();
                playback.render(&mut [[0.0; 2]; 64]).unwrap();
                if removed {
                    editor.execute(Command::RemoveTrack(track)).unwrap();
                    let mut next = SessionPlayback::new(revision(editor.project(), table)).unwrap();
                    next.try_inherit_state_from(&mut playback).unwrap();
                    playback = next;
                } else {
                    playback
                        .apply(SessionPlaybackCommand::StopTrack {
                            track,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
                let mut tail = [[0.0; 2]; 64];
                playback.render(&mut tail).unwrap();
                tail
            };
            let baseline = run(false);
            let actual = run(true);
            assert!(baseline.iter().any(|v| v[0].abs() > 1e-5));
            for (a, b) in actual.iter().zip(baseline) {
                for c in 0..2 {
                    assert!(
                        (a[c] - b[c] * 0.25).abs() < 1e-7,
                        "session={session} removed={removed}: {a:?} vs {b:?}"
                    );
                }
            }
        }
    }
}

#[path = "delay_tests.rs"]
mod delay_tests;

#[test]
fn console_audio_loop_intro_repeats_across_partitions_handoff_and_stop() {
    let source: Vec<Frame> = (0..32)
        .map(|i| [i as f32 / 64.0, -(i as f32) / 64.0])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    for (first, last) in [(7, 13), (7, 8)] {
        let mut snapshot = audio_project(3, 29, 4).snapshot();
        let SessionCellContentSnapshot::Audio(clip) = &mut snapshot.session.cells[0].content else {
            panic!("audio")
        };
        clip.loop_range = Some(hex_project::SessionAudioLoop::new(first, last).unwrap());
        let project = Project::from_snapshot(snapshot).unwrap();
        let media = audio_media(&project, &audio, &audio);
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
        let expected: Vec<_> = (0..97)
            .map(|i| {
                source[if i < last - 3 {
                    3 + i
                } else {
                    first + (i - (last - 3)) % (last - first)
                } as usize]
            })
            .collect();
        for block in [1, 7, 32, 97] {
            let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = vec![[0.0; 2]; 97];
            for chunk in actual.chunks_mut(block) {
                playback.render(chunk).unwrap();
            }
            assert_eq!(actual, expected, "cycle {first}..{last}, block {block}");
            let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            candidate.try_inherit_state_from(&mut playback).unwrap();
            let mut next = [[0.0; 2]; 3];
            candidate.render(&mut next).unwrap();
            for (j, frame) in next.iter().enumerate() {
                assert_eq!(
                    *frame,
                    source[(first + (97 + j as u64 - (last - 3)) % (last - first)) as usize]
                );
            }
            candidate.apply(SessionPlaybackCommand::Stop).unwrap();
            let mut stopped = [[1.0; 2]; 3];
            candidate.render(&mut stopped).unwrap();
            assert_eq!(stopped, [[0.0; 2]; 3]);
        }
    }
}

#[test]
fn mixed_streamed_resident_and_midi_compensation_preserves_page_readiness_and_storage() {
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
            state: limiter,
        })
        .unwrap();
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
    assert_eq!(actual, expected);
    assert!(actual.0.iter().flatten().any(|sample| *sample != 0.0));
    assert!(actual.1.len() > 1);
    assert_eq!(actual.2.len(), 1);
}
