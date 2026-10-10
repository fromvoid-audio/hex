use super::*;
use crate::MidiTiming;
use crate::compile::PreparedAudioSource;
use crate::playback::{Playback, PlaybackCommand};
use crate::{BorrowedMediaPage, MediaPages, StreamSourceKey, StreamedSourceMetadata};
use crate::{PreparedRevision, PreparedSessionInventory};
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipSnapshot, Command, Created, DeviceSnapshot, Editor,
    LoopRegion, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, MidiSourceSnapshot, Project,
    SceneSnapshot, SessionCellContentSnapshot, SessionCellSnapshot, SessionMidiClipSnapshot,
    SessionSnapshot, Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{collections::BTreeMap, sync::Arc};

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
        sample_rate: u32,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
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
}

fn continuity_sample(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    release: u64,
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
        sample_rate: u32,
    ) -> Result<
        Box<dyn hex_lattice::PreparedNativeInstrument>,
        hex_lattice_contract::DeviceContractError,
    > {
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
        Ok(Some(Box::new(MutableFeedbackFactory {
            level: self.level,
            release_frames: self.release_frames,
        })))
    }
}

impl hex_lattice::PreparedMutableInstrumentRuntime for MutableFeedbackFactory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<MutableFeedbackRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        256
    }
    fn max_active_occurrences(&self) -> usize {
        64
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release_frames + 8
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        Box::new(MutableFeedbackRuntime {
            key: self.level.to_bits(),
            level: self.level,
            release_frames: u8::try_from(self.release_frames).unwrap_or(u8::MAX),
            voices: [MutableFeedbackVoice::default(); 64],
            feedback: 0.0,
            tail_left: 0,
        })
    }
}

impl hex_lattice::MutableInstrumentRuntime for MutableFeedbackRuntime {
    fn preflight_events(
        &self,
        events: &[hex_lattice::NativeInstrumentEvent],
        frame_count: usize,
    ) -> Result<(), hex_lattice::NativeInstrumentRuntimeError> {
        let mut voices = self.voices;
        let mut previous_offset = 0;
        for event in events.iter().copied() {
            let offset = match event {
                hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. } => frame_offset,
            };
            if offset < previous_offset || offset >= frame_count {
                return Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            Self::advance(&mut voices, offset - previous_offset);
            match event {
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
    ) {
        let mut event_index = 0;
        for (frame_index, frame) in frames.iter_mut().enumerate() {
            while events.get(event_index).is_some_and(|event| match *event {
                hex_lattice::NativeInstrumentEvent::NoteOn { frame_offset, .. }
                | hex_lattice::NativeInstrumentEvent::NoteOff { frame_offset, .. } => {
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
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| self.key == old.key && self.release_frames == old.release_frames)
    }
    fn copy_history_from(&mut self, previous: &dyn hex_lattice::MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
            self.feedback = old.feedback;
            self.tail_left = old.tail_left;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn wide_analytic_release_and_span_storage_stays_within_derived_fixed_bound() {
    let sampler_bytes = std::mem::size_of::<hex_lattice::PreparedAnalyticNote>();
    assert_eq!(sampler_bytes, 344);
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
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(std::mem::size_of::<super::ReleaseEntry>(), 488);
        assert_eq!(span_buffer_bytes, 82_952);
        assert_eq!(current_bytes, 16_073_736);
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
    assert!(current_bytes < 16 * 1024 * 1024);
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
                id,
                bypassed: false,
                state: state.clone(),
            });
            track.instrument_position = Some(0);
        }
    }
    Project::from_snapshot(snapshot)
}

fn project(gain: f32, note_start: u64, note_duration: u64) -> Project {
    project_at_tempo(gain, note_start, note_duration, Tempo::DEFAULT)
}

fn audio_project(source_start: u64, source_end: u64, asset_id: u64) -> Project {
    legacy_project_from_snapshot(Snapshot {
        sample_rate: 48_000,
        next_id: 7,
        assets: vec![
            AssetSnapshot {
                id: 4,
                path: "shared.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
            AssetSnapshot {
                id: 5,
                path: "replacement.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 32,
                playback_frames: 32,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            },
        ],
        tracks: vec![TrackSnapshot {
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
            id: 8,
            start_raw: 0,
            length_raw: 4 << 32,
            source_length_raw: 4 << 32,
            source_start_raw: 0,
            loop_start_raw: 0,
            loop_end_raw: 4 << 32,
            looping: false,
            notes: vec![MidiNoteSnapshot {
                id: 9,
                start_raw: 0,
                duration_raw: 1 << 32,
                pitch: 69,
                channel: 0,
                velocity: 100,
            }],
        }]),
    });
    snapshot.session.scenes[0].name = "audio and keys".into();
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 6,
        scene: 2,
        content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
            id: 10,
            name: "empty midi".into(),
            source: MidiSourceSnapshot {
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
        id: 11,
        start_raw: 0,
        duration_raw: 1 << 32,
        pitch: 69,
        channel: 0,
        velocity: 100,
    });
    snapshot.tracks.push(TrackSnapshot {
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
    });
    legacy_project_from_snapshot(snapshot)
        .expect("mixed Session and Arrangement audio/MIDI project")
}

fn project_at_tempo(gain: f32, note_start: u64, note_duration: u64, tempo: Tempo) -> Project {
    legacy_project_from_snapshot(Snapshot {
        sample_rate: 48_000,
        next_id: 8,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
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
                id: 2,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    id: 3,
                    start_raw: note_start,
                    duration_raw: note_duration,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            }]),
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
                    id: 5,
                    name: "clip".into(),
                    source: MidiSourceSnapshot {
                        source_length_raw: 32 << 32,
                        source_start_raw: 0,
                        loop_start_raw: 0,
                        loop_end_raw: 32 << 32,
                        looping: false,
                        notes: vec![MidiNoteSnapshot {
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
    })
    .unwrap()
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
        id: 9,
        bypassed: false,
        state,
    });
    snapshot.next_id = 10;
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
        id: 9,
        bypassed: false,
        state,
    });
    old_snapshot.next_id = 10;
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
            id: 9,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            id: 10,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
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
            id: 9,
            bypassed: false,
            state: prefix,
        },
        DeviceSnapshot {
            id: 10,
            bypassed: false,
            state: suffix,
        },
    ];
    snapshot.tracks[0].instrument_position = Some(1);
    snapshot.next_id = 11;
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
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
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
            id: snapshot.next_id,
            bypassed: false,
            state: neutral,
        },
        DeviceSnapshot {
            id: snapshot.next_id + 1,
            bypassed: false,
            state: dc_state,
        },
    ];
    snapshot.next_id += 2;
    snapshot.tracks[0].instrument_position = Some(1);
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let old_revision = revision(&old_project, Arc::clone(&table));

    let render_old_tail =
        |new_project: &Project| {
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
                entry.track_id == next.routes[0].id && entry.tail_group.is_some()
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
        id: instrument_id,
        bypassed: false,
        state: instrument_state,
    });
    snapshot.tracks[0].devices = vec![DeviceSnapshot {
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
            id: snapshot.next_id,
            bypassed: false,
            state: state.clone(),
        },
        hex_project::DeviceSnapshot {
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
fn completed_tail_group_is_retained_on_old_owner_and_its_slot_is_reusable() {
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

    // Model a completed release while retaining its owner until old-graph
    // retirement. The next candidate may reuse its numeric slot, but must not
    // take or drop the completed chain on the callback handoff path.
    let mut next_release = *first.releases.first().unwrap();
    first.releases.clear();
    // Keep a distinct live release on the current track. The next handoff
    // must allocate a tail group for it and can reuse slot zero only if the
    // completed group remains on the retired graph.
    next_release.tail_group = None;
    next_release.track_index = 0;
    first.releases.push(next_release);
    let changed_again = with_utility_filter_state(&base, true, false);
    let mut second = SessionPlayback::new(revision(&changed_again, table)).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    assert!(first.tail_groups[0].is_some());
    assert_eq!(second.tail_groups[0].as_ref().unwrap().stateful_count(), 1);
    assert_eq!(second.tail_groups.iter().flatten().count(), 1);
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
            grouped.tail_group = Some(u16::try_from(slot).unwrap());
            active.releases.push(grouped);
        }
        let mut current_chain_release = release;
        current_chain_release.tail_group = None;
        current_chain_release.track_index = 0;
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
fn stateful_filter_history_freezes_on_pause_resets_on_seek_stop_and_continues_across_loop() {
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
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut silence = [[1.0; 2]; 64];
    paused.render(&mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 64]);
    assert_eq!(paused.performance_frame(), frame);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_pause = [[0.0; 2]; 64];
    paused.render(&mut after_pause).unwrap();
    let mut continuous = start(Arc::clone(&prepared), clip);
    let mut continuous_prefix = [[0.0; 2]; 32];
    continuous.render(&mut continuous_prefix).unwrap();
    let mut continuous_tail = [[0.0; 2]; 64];
    continuous.render(&mut continuous_tail).unwrap();
    assert_eq!(after_pause, continuous_tail);

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
    invalid_tail.track_index = usize::MAX;
    invalid_tail.tail_group = Some(u16::MAX);
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
    legacy_project_from_snapshot(snapshot).unwrap()
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
        id: note_id,
        start_raw: 1 << 32,
        duration_raw: 1 << 32,
        pitch: 64,
        channel: 0,
        velocity: 100,
    });
    Project::from_snapshot(snapshot).unwrap()
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
    legacy_project_from_snapshot(snapshot).unwrap()
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
        .execute(Command::AddMidiTrack {
            name: "short source".into(),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let second_track = match editor
        .execute(Command::AddMidiTrack {
            name: "held source".into(),
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
        .execute(Command::AddMidiTrack {
            name: "removed mutable track".into(),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected first MIDI track"),
    };
    let retained_track = match editor
        .execute(Command::AddMidiTrack {
            name: "retained mutable track".into(),
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
        id: asset,
        path: "mutable-hybrid.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 16,
        playback_frames: 16,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
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
        id: effect,
        bypassed: false,
        state: gain_state,
    }];
    snapshot.tracks[0].instrument_position = Some(0);
    let project = legacy_project_from_snapshot(snapshot).unwrap();
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
                source_start,
                source_gate_end: 8,
                pitch,
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
            hex_lattice::NativeInstrumentEvent::NoteOff { .. } => None,
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
                &mut partitioned_events,
            )
            .unwrap();
    }
    assert_eq!(partitioned_output, whole_output);
    partitioned.commit_block();
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
                source_start,
                source_gate_end: source_start + 1,
                pitch: 60,
                velocity: 100,
            })
            .unwrap();
    }
    let mut events = Vec::new();
    let mut frames = [[0.0; 2]; 256];
    runtime
        .process_segment(track.id(), 0, 0, &mut frames, &mut onsets, &mut events)
        .unwrap();
    assert_eq!(events.len(), 256);
    let mut release = [[0.0; 2]; 16];
    onsets.clear();
    runtime
        .process_segment(track.id(), 256, 256, &mut release, &mut onsets, &mut events)
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
    assert!(active.mutable_instruments[0].is_none());
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| !tail.runtime.committed_is_quiescent())
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
            .any(Option::is_some)
    );
    assert!(
        candidate
            .tail_mutable_instruments
            .iter()
            .flatten()
            .all(|tail| tail.runtime.committed_is_quiescent())
    );
    candidate.apply(SessionPlaybackCommand::Seek(0)).unwrap();
}

#[test]
fn mutable_to_analytic_handoff_refuses_active_chase_then_preserves_old_gate_and_suffix() {
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
            release_frames: 8,
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
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
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
        id: asset,
        path: "mutable-tail-input.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 64,
        playback_frames: 64,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    });
    source.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![ClipSnapshot {
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
    let old_project = legacy_project_from_snapshot(source).unwrap();
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
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
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
    let mut refused = SessionPlayback::new(changed_revision).unwrap();
    let previous_frame = previous.performance_frame();
    assert_eq!(
        refused.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::MutableInstrumentChase)
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
    tail_snapshot.session.cells.clear();
    let tail_project = legacy_project_from_snapshot(tail_snapshot).unwrap();
    let mut tail_candidate =
        SessionPlayback::new(prepare(&tail_project, Arc::clone(&table))).unwrap();
    tail_candidate
        .try_inherit_state_from(&mut reference)
        .unwrap();

    let mut candidate_snapshot = old_project.snapshot();
    if let TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut candidate_snapshot.tracks[0].content
    {
        midi_clips.clear();
    }
    candidate_snapshot.session.cells.clear();
    let candidate_source = legacy_project_from_snapshot(candidate_snapshot).unwrap();
    let empty_analytic = with_test_instrument(&candidate_source, analytic_id, 0);
    let empty_revision = prepare(&empty_analytic, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(empty_revision).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let mut actual = [[0.0; 2]; 16];
    let mut old_tail_pcm = [[0.0; 2]; 16];
    let mut new_audio_pcm = [[0.0; 2]; 16];
    candidate.render(&mut actual).unwrap();
    tail_candidate.render(&mut old_tail_pcm).unwrap();
    let mut fresh_audio =
        SessionPlayback::new(prepare(&candidate_source, Arc::clone(&table))).unwrap();
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
    let mut retained_tail_chase = SessionPlayback::new(later_revision).unwrap();
    let tail_frame = candidate.performance_frame();
    assert_eq!(
        retained_tail_chase.try_inherit_state_from(&mut candidate),
        Err(SessionPlaybackError::MutableInstrumentChase)
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
            release_frames: 8,
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
            id: first_id,
            bypassed: false,
            state: neutral,
        },
        hex_project::DeviceSnapshot {
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
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(previous.performance_frame, previous_frame);
    assert_eq!(previous.routes, previous_routes);
    assert!(
        previous.mutable_instruments[0]
            .as_ref()
            .is_some_and(|runtime| !runtime.committed_is_quiescent())
    );
    for slot in &mut candidate.tail_groups {
        *slot = None;
    }
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
    };
    let note_on = [hex_lattice::NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 60,
        velocity: 100,
    }];
    let mut oracle_prefix = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(&mut oracle, &note_on, &mut oracle_prefix);
    let note_off = [hex_lattice::NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: 1,
    }];
    let mut expected = [[0.0; 2]; 16];
    hex_lattice::MutableInstrumentRuntime::process_block(&mut oracle, &note_off, &mut expected);
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
fn mutable_session_route_switch_refuses_live_state_and_retries_after_quiet() {
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
    let frame_before_refusal = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(playback.performance_frame(), frame_before_refusal);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let mut release = [[0.0; 2]; 32];
    playback.render(&mut release).unwrap();
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
fn mutable_back_to_arrangement_checks_destination_notes_after_quiet_session_tail() {
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
    let before_routes = playback.routes.clone();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(playback.performance_frame(), cut);
    assert_eq!(playback.routes, before_routes);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
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
fn mutable_prepared_playback_refuses_arrangement_and_session_loops() {
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
    assert!(matches!(
        SessionPlayback::new(prepared),
        Err(SessionPlaybackError::MutableInstrumentLoopUnsupported)
    ));

    let mut session_loop = session_only_project(&project(0.5, 0, 1 << 24)).snapshot();
    session_loop.tracks[0].instrument.as_mut().unwrap().state =
        hex_lattice_contract::NativeDeviceState::new(id, 1, vec![0]).unwrap();
    let SessionCellContentSnapshot::Midi(clip) = &mut session_loop.session.cells[0].content else {
        panic!("fixture has a Session MIDI source")
    };
    clip.source.looping = true;
    let session_loop = legacy_project_from_snapshot(session_loop).unwrap();
    let prepared = revision_with_instrument_registry(&session_loop, table, &registry);
    assert!(matches!(
        SessionPlayback::new(prepared),
        Err(SessionPlaybackError::MutableInstrumentLoopUnsupported)
    ));
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
    assert!(
        candidate.releases.iter().any(|entry| {
            entry.track_id == candidate.routes[0].id && entry.continuation.is_some()
        })
    );

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
            id: track,
            name: "new mutable track".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(DeviceSnapshot {
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
                id: arrangement_clip,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    id: arrangement_note,
                    start_raw: arrangement_start,
                    duration_raw: 1 << 32,
                    pitch: 64,
                    channel: 0,
                    velocity: 100,
                }],
            }]),
        });
        snapshot.session.cells.push(SessionCellSnapshot {
            track,
            scene: snapshot.session.scenes[0].id,
            content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                id: session_clip,
                name: "new mutable clip".into(),
                source: MidiSourceSnapshot {
                    source_length_raw: 32 << 32,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: 32 << 32,
                    looping: false,
                    notes: vec![MidiNoteSnapshot {
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
    let bypassed = Project::from_snapshot(bypassed_snapshot).unwrap();

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
        .execute(Command::AddMidiTrack {
            name: "first".into(),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("first MIDI track"),
    };
    let second = match editor
        .execute(Command::AddMidiTrack {
            name: "second".into(),
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
            id: 6,
            start_raw: 5_u64 << 31, // loop phase 1/2; first repeat begins at 3.5
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 90,
        }];
    }
    snapshot.next_id = 9;
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
    let launch = match old.routes[0].owner {
        RouteOwner::Session { launch, .. } => launch,
        _ => panic!("Session source is selected"),
    };
    old.revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query(launch, &old.timing_view(), 100_000, 100_001, &mut old_spans)
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
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();

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
    let mut candidate = SessionPlayback::new(revision(&slower, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.anchor.is_some());
    let before = candidate.performance_frame();
    candidate.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused = [[7.0; 2]; 32];
    candidate.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 32]);
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
        matches!(candidate.routes[0].owner, RouteOwner::Session { launch, .. } if launch == BeatTime::ZERO)
    );
}

#[test]
fn failed_handoff_capacity_is_atomic_and_can_be_retried() {
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
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();
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
                entry.track_index,
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
                    entry.track_index,
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
    let RouteOwner::Session { launch, .. } = candidate.routes[0].owner else {
        panic!("due launch selects the Session clip");
    };
    assert_eq!(launch, effective_cut);
    let mut spans = MidiSpanBuffer::default();
    candidate
        .revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query(launch, &candidate.timing_view(), 96_000, 96_001, &mut spans)
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
    let mut old = SessionPlayback::new(old_revision).unwrap();
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
    let mut pages = MissingPages::default();
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        failed.render_with_pages(&mut output, &mut pages),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(pages.0, [(stream_source.key(), 0)]);
    assert_eq!(failed.performance_frame(), 1);
    assert_eq!(failed.arrangement_frame(), 1);
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
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
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
    for (track_id, clip_id, note_id, gain) in [(7, 9, 14, 0.25), (10, 11, 12, 0.75)] {
        snapshot.tracks.push(TrackSnapshot {
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
                id: clip_id,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    id: note_id,
                    start_raw: 0,
                    duration_raw: 8 << 32,
                    pitch: 60 + track_id as u8,
                    channel: 0,
                    velocity: 90,
                }],
            }]),
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
        if let Some(sample) = deleted_tail.voice.sample_at(absolute) {
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
                id: 3,
                start_raw: 3_u64 << 31, // intro onset, gate crosses the first loop seam
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
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
                id: 14,
                start_raw: 3_u64 << 31,
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            MidiNoteSnapshot {
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
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut snapshot = original.snapshot();
    snapshot.tracks.push(TrackSnapshot {
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
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    assert_eq!(
        playback.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(playback.release_voice_count(), 1);
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
    assert_eq!(route.active_launch, None);
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
    assert_eq!(route.active_launch, Some(BeatTime::ZERO));

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
        playback.route_status(0).unwrap().active_launch,
        Some(BeatTime::ZERO)
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
