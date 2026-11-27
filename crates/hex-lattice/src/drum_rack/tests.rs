use super::*;
use crate::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, EMPTY_DEVICE_RESOURCES,
    InstrumentRackBranchSpec, InstrumentRackSpec, InstrumentRackZone, TuningTable,
};
use std::num::{NonZeroU8, NonZeroU64};

const TEST_PAD_SYNTH_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.drum.pad.synth");
const TEST_PAD_SAMPLE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.drum.pad.sample");

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

fn choke(v: u8) -> Option<NonZeroU8> {
    NonZeroU8::new(v)
}

fn synth_state(level: f32, latency: u32) -> NativeDeviceState {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&level.to_le_bytes());
    bytes.extend_from_slice(&latency.to_le_bytes());
    NativeDeviceState::new(TEST_PAD_SYNTH_TYPE, 1, bytes).unwrap()
}

fn sample_pad_state(asset_key: u64) -> NativeDeviceState {
    NativeDeviceState::new(TEST_PAD_SAMPLE_TYPE, 1, vec![1])
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset_key).unwrap(),
        }])
        .unwrap()
}

/// Deterministic polyphonic test pad instrument whose output is
/// `level * (pitch / 100.0) * (velocity / 127.0)` per active voice.
struct TestPadSynthDef;

struct TestPadSynthPrepared {
    level: f32,
    latency: u32,
}

#[derive(Clone, Copy)]
struct ActivePadVoice {
    occurrence_id: u64,
    amplitude: f32,
}

struct TestPadSynthRuntime {
    level: f32,
    latency: u32,
    voices: [Option<ActivePadVoice>; 16],
}

impl NativeDeviceMetadata for TestPadSynthDef {
    fn type_id(&self) -> DeviceTypeId {
        TEST_PAD_SYNTH_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(synth_state(1.0, 0))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
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

impl NativeInstrumentDefinition for TestPadSynthDef {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        context.validate_for(state)?;
        let bytes = state.payload();
        if bytes.len() != 8 {
            return Err(DeviceContractError::MalformedState);
        }
        let level = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let latency = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Ok(Box::new(TestPadSynthPrepared { level, latency }))
    }
}

impl PreparedNativeInstrument for TestPadSynthPrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.latency)
    }
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(TestPadSynthPrepared {
            level: self.level,
            latency: self.latency,
        })))
    }
}

impl PreparedMutableInstrumentRuntime for TestPadSynthPrepared {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn steals_releasing_voices(&self) -> bool {
        false
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.latency)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<TestPadSynthRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        64
    }
    fn max_active_occurrences(&self) -> usize {
        16
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(TestPadSynthRuntime {
            level: self.level,
            latency: self.latency,
            voices: [None; 16],
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for TestPadSynthRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let mut count = 0_usize;
        for voice in self.voices.iter().flatten() {
            if count >= output.len() {
                return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
            }
            output[count] = voice.occurrence_id;
            count += 1;
        }
        output[..count].sort_unstable();
        Ok(count)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > 64 {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut active = [0_u64; 16];
        let mut active_len = 0_usize;
        for v in self.voices.iter().flatten() {
            active[active_len] = v.occurrence_id;
            active_len += 1;
        }
        for &ev in events {
            if ev.frame_offset() >= frame_count {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            match ev {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if !active[..active_len].contains(&occurrence_id) {
                        if active_len >= active.len() {
                            return Err(
                                NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded,
                            );
                        }
                        active[active_len] = occurrence_id;
                        active_len += 1;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let Some(pos) = active[..active_len]
                        .iter()
                        .position(|&id| id == occurrence_id)
                    else {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    };
                    active[pos] = active[active_len - 1];
                    active_len -= 1;
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
        let mut ev_idx = 0_usize;
        for (frame_idx, out) in frames.iter_mut().enumerate() {
            while ev_idx < events.len() && events[ev_idx].frame_offset() == frame_idx {
                match events[ev_idx] {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        velocity,
                        ..
                    } => {
                        let amp =
                            self.level * (f32::from(pitch) / 100.0) * (f32::from(velocity) / 127.0);
                        if let Some(slot) = self.voices.iter_mut().find(|v| v.is_none()) {
                            *slot = Some(ActivePadVoice {
                                occurrence_id,
                                amplitude: amp,
                            });
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        for slot in &mut self.voices {
                            if slot.is_some_and(|v| v.occurrence_id == occurrence_id) {
                                *slot = None;
                            }
                        }
                    }
                    _ => {}
                }
                ev_idx += 1;
            }
            let mut sum = 0.0_f32;
            for v in self.voices.iter().flatten() {
                sum += v.amplitude;
            }
            out[0] += sum;
            out[1] += sum;
        }
    }
    fn reset(&mut self) {
        self.voices.fill(None);
    }
    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(Option::is_none)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o.level.to_bits() == self.level.to_bits() && o.latency == self.latency)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(other) = previous.as_any().downcast_ref::<Self>() {
            self.voices = other.voices;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Sample-backed test pad requiring `DeviceAssetSlot::new(1)` from `DeviceResources`.
struct TestSamplePadDef;

struct TestSamplePadPrepared {
    identity: DeviceResourceIdentity,
}

struct TestSamplePadRuntime {
    identity: DeviceResourceIdentity,
    active: Option<(u64, usize)>,
}

impl NativeDeviceMetadata for TestSamplePadDef {
    fn type_id(&self) -> DeviceTypeId {
        TEST_PAD_SAMPLE_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(NativeDeviceState::new(TEST_PAD_SAMPLE_TYPE, 1, vec![1]).unwrap())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
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

impl NativeInstrumentDefinition for TestSamplePadDef {
    fn supports_asset_resources(&self) -> bool {
        true
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        context.validate_for(state)?;
        let slot = DeviceAssetSlot::new(1)?;
        let view = context
            .resources
            .slot(slot)
            .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
        Ok(Box::new(TestSamplePadPrepared {
            identity: view.identity,
        }))
    }
}

impl PreparedNativeInstrument for TestSamplePadPrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(TestSamplePadPrepared {
            identity: self.identity,
        })))
    }
}

impl PreparedMutableInstrumentRuntime for TestSamplePadPrepared {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn steals_releasing_voices(&self) -> bool {
        false
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<TestSamplePadRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        64
    }
    fn max_active_occurrences(&self) -> usize {
        8
    }
    fn max_release_tail_frames(&self) -> u64 {
        0
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(TestSamplePadRuntime {
            identity: self.identity,
            active: None,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for TestSamplePadRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        if let Some((occ, _)) = self.active {
            if output.is_empty() {
                return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
            }
            output[0] = occ;
            Ok(1)
        } else {
            Ok(0)
        }
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let Some(view) = resources.slot(DeviceAssetSlot::new(1).unwrap()) else {
            return Err(NativeInstrumentRuntimeError::ResourceUnavailable);
        };
        if view.identity != self.identity {
            return Err(NativeInstrumentRuntimeError::ResourceUnavailable);
        }
        for &ev in events {
            if ev.frame_offset() >= frame_count {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        let Some(view) = resources.slot(DeviceAssetSlot::new(1).unwrap()) else {
            return;
        };
        let mut ev_idx = 0_usize;
        for (i, out) in frames.iter_mut().enumerate() {
            while ev_idx < events.len() && events[ev_idx].frame_offset() == i {
                match events[ev_idx] {
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        self.active = Some((occurrence_id, 0));
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if self.active.is_some_and(|(o, _)| o == occurrence_id) {
                            self.active = None;
                        }
                    }
                    _ => {}
                }
                ev_idx += 1;
            }
            if let Some((_, pos)) = &mut self.active
                && let Some(sample) = view.pcm.frame(*pos % view.pcm.frames())
            {
                out[0] += sample[0];
                out[1] += sample[1];
                *pos += 1;
            }
        }
    }
    fn reset(&mut self) {
        self.active = None;
    }
    fn is_quiescent(&self) -> bool {
        self.active.is_none()
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o.identity == self.identity)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(other) = previous.as_any().downcast_ref::<Self>() {
            self.active = other.active;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn child_registry() -> NativeInstrumentRegistry<'static> {
    static SYNTH: TestPadSynthDef = TestPadSynthDef;
    static SAMPLE: TestSamplePadDef = TestSamplePadDef;
    static DEFS: [&dyn NativeInstrumentDefinition; 2] = [&SYNTH, &SAMPLE];
    NativeInstrumentRegistry::new(&DEFS).unwrap()
}

fn context<'a>(resources: &'a dyn DeviceResources) -> DevicePreparationContext<'a> {
    static TUNING: std::sync::LazyLock<TuningTable> =
        std::sync::LazyLock::new(TuningTable::default);
    DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &TUNING,
        resources,
    }
}

fn make_resource_view<'a>(
    outer_slot: u32,
    asset_key: u64,
    pcm: &'a [[f32; 2]],
) -> DeviceResourceView<'a> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(outer_slot).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(asset_key).unwrap(),
            content: [asset_key as u8; 32],
            derivation_version: 1,
            derivation: [1; 32],
        },
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Stereo(pcm),
    }
}

#[test]
fn two_pad_routing_play_note_velocity_same_pitch_overlap_and_unmapped_notes() {
    let children = child_registry();
    let rack_def = DrumRackDefinition::new(&children).unwrap();

    let spec = DrumRackSpec {
        rack_id: id(10),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(101),
                name: "kick".into(),
                receive_note: 36,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.5, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(102),
                name: "snare".into(),
                receive_note: 38,
                play_note: 80,
                choke_group: None,
                gain: 0.5,
                muted: false,
                state: Some(synth_state(1.0, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(3),
                device_id: id(103),
                name: "muted-hat".into(),
                receive_note: 42,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: true,
                state: Some(synth_state(1.0, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(4),
                device_id: id(104),
                name: "empty-pad".into(),
                receive_note: 44,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: None,
            },
        ],
    };
    let encoded = spec.encode_state().unwrap();
    let decoded = DrumRackSpec::decode_and_migrate_state(&encoded).unwrap();
    assert_eq!(decoded, spec);

    let prepared = rack_def
        .prepare_instrument(&encoded, &context(&EMPTY_DEVICE_RESOURCES))
        .unwrap();
    let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
    let mut runtime = factory.create_runtime();
    assert!(runtime.fixed_storage_bytes() <= factory.declared_twin_bytes());

    // Frame 0: kick (36->60, vel 127 => 0.5*0.60*1.0 = 0.30) + unmapped 50 + muted 42 + empty 44
    // Frame 2: same-pitch overlap kick (36->60, vel 63 => 0.5*0.60*(63/127))
    // Frame 4: snare (38->80, vel 127 => gain 0.5 * 1.0 * 0.80 = 0.40)
    // Frame 6: note-off first kick (occ 1) only -> second kick + snare keep sounding
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 36,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 90,
            pitch: 50,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 91,
            pitch: 42,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 92,
            pitch: 44,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 2,
            occurrence_id: 2,
            pitch: 36,
            velocity: 63,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 4,
            occurrence_id: 3,
            pitch: 38,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 6,
            occurrence_id: 1,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 6,
            occurrence_id: 90,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 6,
            occurrence_id: 91,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 6,
            occurrence_id: 92,
        },
    ];
    runtime
        .preflight_events(&events, 8, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut frames = [[0.0_f32; 2]; 8];
    runtime.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES);

    let kick_1 = 0.5 * 0.60;
    let kick_2 = 0.5 * 0.60 * (63.0 / 127.0);
    let snare = 0.5 * 0.80;
    assert!((frames[0][0] - kick_1).abs() < 1e-6);
    assert!((frames[2][0] - (kick_1 + kick_2)).abs() < 1e-6);
    assert!((frames[4][0] - (kick_1 + kick_2 + snare)).abs() < 1e-6);
    assert!((frames[6][0] - (kick_2 + snare)).abs() < 1e-6);

    let mut active = [0_u64; 8];
    let n = runtime.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..n], &[2, 3]);

    // Stop/reset clears all voices to quiescence.
    runtime.reset();
    assert!(runtime.is_quiescent());
}

#[test]
fn choke_group_cuts_other_pad_at_exact_frame_without_orphaned_note_off() {
    let children = child_registry();
    let rack_def = DrumRackDefinition::new(&children).unwrap();

    let spec = DrumRackSpec {
        rack_id: id(11),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(101),
                name: "open-hat".into(),
                receive_note: 46,
                play_note: 100,
                choke_group: choke(1),
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.5, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(102),
                name: "closed-hat".into(),
                receive_note: 42,
                play_note: 100,
                choke_group: choke(1),
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.25, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(3),
                device_id: id(103),
                name: "kick".into(),
                receive_note: 36,
                play_note: 100,
                choke_group: choke(2),
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.1, 0)),
            },
        ],
    };
    let encoded = spec.encode_state().unwrap();
    let prepared = rack_def
        .prepare_instrument(&encoded, &context(&EMPTY_DEVICE_RESOURCES))
        .unwrap();
    let mut runtime = prepared
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
        .create_runtime();

    // Frame 0: open-hat (0.5) + kick (0.1) => 0.6
    // Frame 3: closed-hat (0.25) in group 1 chokes open-hat immediately, kick stays => 0.35
    // Frame 6: clip NoteOff for already-choked open-hat (occ 1) must NOT cause OrphanedNoteOff!
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 46,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 36,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 3,
            occurrence_id: 3,
            pitch: 42,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 6,
            occurrence_id: 1,
        },
    ];
    runtime
        .preflight_events(&events, 8, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut frames = [[0.0_f32; 2]; 8];
    runtime.process_block(&events, &mut frames, &EMPTY_DEVICE_RESOURCES);

    assert!((frames[2][0] - 0.6).abs() < 1e-6);
    assert!((frames[3][0] - 0.35).abs() < 1e-6);
    assert!((frames[7][0] - 0.35).abs() < 1e-6);

    let mut active = [0_u64; 8];
    let n = runtime.active_occurrence_ids(&mut active).unwrap();
    assert_eq!(&active[..n], &[2, 3]);
}

#[test]
fn two_sample_backed_pads_remap_distinct_assets_and_refuse_missing_asset() {
    let children = child_registry();
    let rack_def = DrumRackDefinition::new(&children).unwrap();

    let spec = DrumRackSpec {
        rack_id: id(12),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(201),
                name: "sample-a".into(),
                receive_note: 36,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(sample_pad_state(1001)),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(202),
                name: "sample-b".into(),
                receive_note: 38,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(sample_pad_state(1002)),
            },
        ],
    };
    let encoded = spec.encode_state().unwrap();
    assert_eq!(encoded.asset_references().len(), 2);
    assert_eq!(encoded.asset_references()[0].slot.value(), 1);
    assert_eq!(encoded.asset_references()[0].asset.value(), 1001);
    assert_eq!(encoded.asset_references()[1].slot.value(), 2);
    assert_eq!(encoded.asset_references()[1].asset.value(), 1002);

    // Missing asset refused cleanly on control.
    assert!(matches!(
        rack_def.prepare_instrument(&encoded, &context(&EMPTY_DEVICE_RESOURCES)),
        Err(DeviceContractError::AssetResourcesUnavailable)
    ));

    let pcm_a = [[0.25, 0.25], [0.5, 0.5]];
    let pcm_b = [[-0.125, -0.125], [-0.25, -0.25]];
    let views = [
        make_resource_view(1, 1001, &pcm_a),
        make_resource_view(2, 1002, &pcm_b),
    ];
    let resources = BorrowedDeviceResources(&views);

    let prepared = rack_def
        .prepare_instrument(&encoded, &context(&resources))
        .unwrap();
    let mut runtime = prepared
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
        .create_runtime();

    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 36,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 38,
            velocity: 127,
        },
    ];
    runtime.preflight_events(&events, 2, &resources).unwrap();
    let mut frames = [[0.0_f32; 2]; 2];
    runtime.process_block(&events, &mut frames, &resources);
    assert!((frames[0][0] - 0.125).abs() < 1e-6);
    assert!((frames[1][0] - 0.25).abs() < 1e-6);
}

#[test]
fn live_pad_rename_remap_and_idle_removal_retarget_without_stuck_notes() {
    let children = child_registry();
    let rack_def = DrumRackDefinition::new(&children).unwrap();

    let mut spec = DrumRackSpec {
        rack_id: id(13),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(301),
                name: "kick".into(),
                receive_note: 36,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.5, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(302),
                name: "snare".into(),
                receive_note: 38,
                play_note: 60,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.25, 0)),
            },
        ],
    };
    let state_a = spec.encode_state().unwrap();
    let factory_a = rack_def
        .prepare_instrument(&state_a, &context(&EMPTY_DEVICE_RESOURCES))
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let mut rt_a = factory_a.create_runtime();

    // Hold note on pad 1 (receive_note 36, occurrence 10).
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 10,
        pitch: 36,
        velocity: 127,
    }];
    let mut out = [[0.0; 2]; 4];
    rt_a.preflight_events(&on, 4, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    rt_a.process_block(&on, &mut out, &EMPTY_DEVICE_RESOURCES);
    assert!((out[0][0] - 0.3).abs() < 1e-6);

    // Rename pad 1, remap receive_note 36 -> 40, and remove idle pad 2!
    spec.rename_pad(id(1), "sub-kick").unwrap();
    spec.remap_pad(id(1), 40, 60).unwrap();
    spec.remove_pad(id(2)).unwrap();
    let state_b = spec.encode_state().unwrap();
    let factory_b = rack_def
        .prepare_instrument(&state_b, &context(&EMPTY_DEVICE_RESOURCES))
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        factory_b.prepare_history_retarget_target().unwrap()
    else {
        panic!("expected retarget proof");
    };
    let mut rt_b = factory_b.create_runtime();
    assert!(rt_b.can_retarget_history_from(rt_a.as_ref(), proof.as_ref()));
    rt_b.retarget_history_from(rt_a.as_ref(), proof.as_ref());

    // NoteOff for occurrence 10 still releases held voice cleanly even after remap!
    let off = [NativeInstrumentEvent::NoteOff {
        frame_offset: 2,
        occurrence_id: 10,
    }];
    let mut out_b = [[0.0; 2]; 4];
    rt_b.preflight_events(&off, 4, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    rt_b.process_block(&off, &mut out_b, &EMPTY_DEVICE_RESOURCES);
    assert!((out_b[1][0] - 0.3).abs() < 1e-6);
    assert_eq!(out_b[2][0], 0.0);
    assert!(rt_b.is_quiescent());

    // Removing a pad that STILL holds an active voice refuses live retargeting.
    let mut spec_removed_active = spec.clone();
    spec_removed_active.remove_pad(id(1)).unwrap();
    let factory_c = rack_def
        .prepare_instrument(
            &spec_removed_active.encode_state().unwrap(),
            &context(&EMPTY_DEVICE_RESOURCES),
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let InstrumentHistoryRetargetPreparation::Prepared(proof_c) =
        factory_c.prepare_history_retarget_target().unwrap()
    else {
        panic!("expected proof");
    };
    let rt_c = factory_c.create_runtime();
    assert!(!rt_c.can_retarget_history_from(rt_a.as_ref(), proof_c.as_ref()));
}

#[test]
fn pdc_alignment_validation_and_malformed_state_refusals() {
    let children = child_registry();
    let rack_def = DrumRackDefinition::new(&children).unwrap();

    // PDC: pad 0 latency 0, pad 1 latency 3 -> rack latency 3, both align at frame 3.
    let pdc_spec = DrumRackSpec {
        rack_id: id(14),
        gain: 1.0,
        pads: vec![
            DrumRackPadSpec {
                pad_id: id(1),
                device_id: id(401),
                name: "dry".into(),
                receive_note: 36,
                play_note: 100,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.5, 0)),
            },
            DrumRackPadSpec {
                pad_id: id(2),
                device_id: id(402),
                name: "delayed".into(),
                receive_note: 38,
                play_note: 100,
                choke_group: None,
                gain: 1.0,
                muted: false,
                state: Some(synth_state(0.25, 3)),
            },
        ],
    };
    let prepared = rack_def
        .prepare_instrument(
            &pdc_spec.encode_state().unwrap(),
            &context(&EMPTY_DEVICE_RESOURCES),
        )
        .unwrap();
    assert_eq!(prepared.algorithmic_latency_frames(), Some(3));
    let mut rt = prepared
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap()
        .create_runtime();
    let on = [NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 36,
        velocity: 127,
    }];
    let mut frames = [[0.0; 2]; 5];
    rt.process_block(&on, &mut frames, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(frames[0][0], 0.0);
    assert_eq!(frames[2][0], 0.0);
    assert!((frames[3][0] - 0.5).abs() < 1e-6);

    // Duplicate receive_note refused.
    let mut dup_note = pdc_spec.clone();
    dup_note.pads[1].receive_note = 36;
    assert_eq!(
        dup_note.validate(),
        Err(DeviceContractError::InvalidParameterValue)
    );

    // Choke group > 16 refused.
    let mut bad_choke = pdc_spec.clone();
    bad_choke.pads[0].choke_group = NonZeroU8::new(17);
    assert_eq!(
        bad_choke.validate(),
        Err(DeviceContractError::InvalidParameterValue)
    );

    // Nested Instrument Rack refused.
    let nested_ir = InstrumentRackSpec {
        rack_id: id(99),
        overlap_policy: crate::InstrumentRackOverlapPolicy::Layer,
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(1),
                gain: 1.0,
                bypassed: false,
                zone: InstrumentRackZone::default(),
                state: None,
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(2),
                gain: 1.0,
                bypassed: false,
                zone: InstrumentRackZone::default(),
                state: None,
            },
        ],
    }
    .encode_state()
    .unwrap();
    let mut nested = pdc_spec.clone();
    nested.pads[0].state = Some(nested_ir);
    assert_eq!(
        nested.validate(),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );

    // Unsupported future state version refused.
    let future = NativeDeviceState::new(DRUM_RACK_TYPE_ID, 2, vec![0; 16]).unwrap();
    assert_eq!(
        DrumRackSpec::decode_and_migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );

    // Trailing garbage bytes refused.
    let good = pdc_spec.encode_state().unwrap();
    let mut trailing = good.payload().to_vec();
    trailing.push(0xFF);
    let bad = NativeDeviceState::new(DRUM_RACK_TYPE_ID, 1, trailing).unwrap();
    assert_eq!(
        DrumRackSpec::decode_and_migrate_state(&bad),
        Err(DeviceContractError::MalformedState)
    );

    // Stale pad_id edit refused cleanly.
    let mut edit_target = pdc_spec;
    assert_eq!(
        edit_target.rename_pad(id(999), "ghost"),
        Err(DeviceContractError::InvalidId)
    );
}
