//! Zero-allocation/free probe for callback retarget release-deadline
//! reconciliation and extended-tail consumption (Pulse #6833).
#![cfg(feature = "allocation-probe")]

use hex_audio::prepared;
use hex_engine::playback::{Playback, PlaybackCommand};
use hex_engine::{PreparedRevision, SineTable};
use hex_lattice::{
    DevicePreparationContext, DeviceResources, InstrumentHistoryRetargetPreparation,
    MutableInstrumentRuntime, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentOccurrenceGain, NativeInstrumentRegistry, NativeInstrumentRuntimeError,
    PreparedAnalyticNote, PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
    PreparedNativeInstrument, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("test.instrument.retarget-release");
const RATE: u32 = 48_000;
const VOICES: usize = 8;

struct Definition;

#[derive(Clone, Copy)]
struct Factory {
    release_frames: u64,
}

#[derive(Clone, Copy)]
struct Proof {
    release_frames: u64,
}

impl PreparedInstrumentHistoryRetarget for Proof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    held: bool,
    release_elapsed: u64,
}

struct Runtime {
    release_frames: u64,
    voices: [Voice; VOICES],
}

fn state_for_release(release_frames: u64) -> NativeDeviceState {
    NativeDeviceState::new(TYPE_ID, 1, release_frames.to_le_bytes().to_vec()).unwrap()
}

impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("retarget release")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(TYPE_ID, 1, 8_u64.to_le_bytes().to_vec())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TYPE_ID || state.payload().len() != 8 {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl NativeInstrumentDefinition for Definition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.migrate_state(state)?;
        let bytes: [u8; 8] = state
            .payload()
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(Box::new(Factory {
            release_frames: u64::from_le_bytes(bytes),
        }))
    }
}

impl PreparedNativeInstrument for Factory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(*self)))
    }
}

impl PreparedMutableInstrumentRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        259
    }
    fn max_active_occurrences(&self) -> usize {
        VOICES
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.release_frames
    }
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime {
            release_frames: self.release_frames,
            voices: [Voice::default(); VOICES],
        })
    }
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Proof {
                release_frames: self.release_frames,
            },
        )))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for Runtime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn preflight_occurrence_gains(
        &self,
        _: &[NativeInstrumentEvent],
        _: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let count = self.voices.iter().filter(|v| v.id != 0).count();
        if count > output.len() {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        for (dst, voice) in output
            .iter_mut()
            .zip(self.voices.iter().filter(|v| v.id != 0))
        {
            *dst = voice.id;
        }
        Ok(count)
    }
    fn preflight_events(
        &self,
        _: &[NativeInstrumentEvent],
        _: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        output: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, output, None, &[], resources);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        output: &mut [StereoFrame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        _: &[NativeInstrumentOccurrenceGain],
        _: &dyn DeviceResources,
    ) {
        output.fill([0.0; 2]);
        let mut cursor = 0usize;
        for (offset, frame) in output.iter_mut().enumerate() {
            while cursor < events.len() && events[cursor].frame_offset() == offset {
                match events[cursor] {
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if let Some(v) = self.voices.iter_mut().find(|v| v.id == occurrence_id) {
                            v.held = false;
                            v.release_elapsed = 0;
                            if self.release_frames == 0 {
                                *v = Voice::default();
                            }
                        }
                    }
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        if let Some(slot) = self.voices.iter_mut().find(|v| v.id == 0) {
                            *slot = Voice {
                                id: occurrence_id,
                                held: true,
                                release_elapsed: 0,
                            };
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            let mut sum = 0.0_f32;
            for v in &mut self.voices {
                if v.id == 0 {
                    continue;
                }
                if !v.held && v.release_elapsed >= self.release_frames {
                    *v = Voice::default();
                    continue;
                }
                sum += 0.25;
                if !v.held {
                    v.release_elapsed += 1;
                    if v.release_elapsed >= self.release_frames {
                        *v = Voice::default();
                    }
                }
            }
            *frame = [sum; 2];
        }
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|v| v.id == 0)
    }
    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.release_frames == self.release_frames)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
        }
    }
    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        previous.as_any().is::<Self>()
            && proof
                .as_any()
                .downcast_ref::<Proof>()
                .is_some_and(|p| p.release_frames == self.release_frames)
    }
    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        _: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.voices = old.voices;
            for v in &mut self.voices {
                if v.id != 0 && !v.held && v.release_elapsed >= self.release_frames {
                    *v = Voice::default();
                }
            }
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[test]
fn callback_retarget_lengthening_release_allocates_and_frees_nothing() {
    let definition = Definition;
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(SineTable::new());

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "retarget probe".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: definition.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let instrument_id = editor.project().tracks()[0].instrument().unwrap().id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    // 4-frame note at 120 BPM (24_000 frames/quarter): 4 / 24_000 quarters.
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw((1 << 32) / 6_000),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();

    let initial_rev = Arc::new(
        PreparedRevision::prepare_with_test_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            None,
            &registry,
        )
        .unwrap(),
    );
    let initial_playback =
        Playback::try_from_prepared_with_session(Arc::clone(&initial_rev)).unwrap();

    let longer_state = state_for_release(48);
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument_id,
            state: longer_state,
        })
        .unwrap();
    let longer_rev = Arc::new(
        PreparedRevision::prepare_with_test_instrument_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(initial_rev.session()),
            &registry,
        )
        .unwrap(),
    );
    let longer_playback = Playback::try_from_prepared_with_session(longer_rev).unwrap();

    let (mut control, mut callback) = prepared::channel(RATE).unwrap();
    control.publish(initial_playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();

    // Render 8 frames: note starts at 0, releases at 4, 4 frames into its initial 8-frame tail.
    let mut prefix = [[0.0; 2]; 8];
    callback.begin_block();
    callback.render(&mut prefix).unwrap();
    assert!(prefix.iter().all(|f| f[0] > 0.0));

    // Publish longer-release candidate (48 frames -> audible through frame 52).
    control.publish(longer_playback).unwrap();

    let mut retarget_and_tail = [[0.0; 2]; 52];
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut retarget_and_tail[..24]).unwrap();
        callback.begin_block();
        callback.render(&mut retarget_and_tail[24..]).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current,
        ),
        (0, 0, 0, 0)
    );
    // Frames 8..52 (indices 0..44) remain audible through the extended terminal;
    // frames 52..60 (indices 44..52) are completely silent.
    assert!(retarget_and_tail[..44].iter().all(|f| f[0] > 0.0));
    assert!(retarget_and_tail[44..].iter().all(|f| *f == [0.0; 2]));
}
