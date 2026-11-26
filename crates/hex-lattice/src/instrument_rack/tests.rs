use super::*;
use crate::{EMPTY_DEVICE_RESOURCES, NativeInstrumentRuntimePair, TuningTable};

const VOICE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.voice");

#[derive(Clone, Copy)]
struct VoiceConfig {
    level: f32,
    latency_frames: u32,
    release_frames: u32,
}

fn encode_voice(cfg: VoiceConfig) -> NativeDeviceState {
    let mut payload = Vec::with_capacity(12);
    payload.extend_from_slice(&cfg.level.to_le_bytes());
    payload.extend_from_slice(&cfg.latency_frames.to_le_bytes());
    payload.extend_from_slice(&cfg.release_frames.to_le_bytes());
    NativeDeviceState::new(VOICE_TYPE, 1, payload).unwrap()
}

fn decode_voice(state: &NativeDeviceState) -> Result<VoiceConfig, DeviceContractError> {
    if state.type_id() != VOICE_TYPE {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != 1 {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let p = state.payload();
    if p.len() != 12 {
        return Err(DeviceContractError::MalformedState);
    }
    let level = f32::from_le_bytes(p[0..4].try_into().unwrap());
    let latency_frames = u32::from_le_bytes(p[4..8].try_into().unwrap());
    let release_frames = u32::from_le_bytes(p[8..12].try_into().unwrap());
    if !level.is_finite() || !(0.0..=2.0).contains(&level) || latency_frames > 64 {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(VoiceConfig {
        level,
        latency_frames,
        release_frames,
    })
}

struct TestVoiceDef;

impl NativeDeviceMetadata for TestVoiceDef {
    fn type_id(&self) -> DeviceTypeId {
        VOICE_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(encode_voice(VoiceConfig {
            level: 0.5,
            latency_frames: 0,
            release_frames: 4,
        }))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let cfg = decode_voice(state)?;
        Ok(encode_voice(cfg))
    }
    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl NativeInstrumentDefinition for TestVoiceDef {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let cfg = decode_voice(state)?;
        Ok(Box::new(PreparedTestVoice(cfg)))
    }
}

struct PreparedTestVoice(VoiceConfig);

impl PreparedNativeInstrument for PreparedTestVoice {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.0.latency_frames)
    }
    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(PreparedTestVoiceFactory(self.0))))
    }
}

struct PreparedTestVoiceFactory(VoiceConfig);

impl PreparedMutableInstrumentRuntime for PreparedTestVoiceFactory {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }
    fn supports_occurrence_gain(&self) -> bool {
        true
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.0.latency_frames)
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<TestVoiceRuntime>()
    }
    fn max_events_per_block(&self) -> usize {
        259
    }
    fn max_active_occurrences(&self) -> usize {
        16
    }
    fn max_release_tail_frames(&self) -> u64 {
        u64::from(self.0.release_frames) + u64::from(self.0.latency_frames)
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(TestVoiceRuntime {
            cfg: self.0,
            voices: [None; 16],
            delay: [[0.0; 2]; 64],
            next: 0,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct VoiceSlot {
    id: u64,
    pitch: u8,
    released: bool,
    release_left: u32,
}

struct TestVoiceRuntime {
    cfg: VoiceConfig,
    voices: [Option<VoiceSlot>; 16],
    delay: [[f32; 2]; 64],
    next: usize,
}

impl MutableInstrumentRuntime for TestVoiceRuntime {
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
        let mut ids = [0_u64; 16];
        let mut len = 0;
        for v in self.voices.iter().flatten() {
            ids[len] = v.id;
            len += 1;
        }
        ids[..len].sort_unstable();
        if len > output.len() {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        output[..len].copy_from_slice(&ids[..len]);
        Ok(len)
    }
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _frame_count: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let mut sim = self.voices;
        for &event in events {
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    ..
                } => {
                    if sim.iter().flatten().any(|v| v.id == occurrence_id) {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    let free = sim
                        .iter()
                        .position(Option::is_none)
                        .ok_or(NativeInstrumentRuntimeError::EventCapacityExceeded)?;
                    sim[free] = Some(VoiceSlot {
                        id: occurrence_id,
                        pitch,
                        released: false,
                        release_left: self.cfg.release_frames,
                    });
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = sim
                        .iter_mut()
                        .find(|slot| slot.is_some_and(|v| v.id == occurrence_id && !v.released))
                        .ok_or(NativeInstrumentRuntimeError::InvalidEventBatch)?;
                    if let Some(v) = slot.as_mut() {
                        v.released = true;
                        if v.release_left == 0 {
                            *slot = None;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn preflight_occurrence_gains(
        &self,
        _events: &[NativeInstrumentEvent],
        _gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn crate::DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, None, &[], resources);
    }
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        _resources: &dyn crate::DeviceResources,
    ) {
        let mut event_idx = 0;
        let lat = self.cfg.latency_frames as usize;
        for (offset, out) in frames.iter_mut().enumerate() {
            while event_idx < events.len() && events[event_idx].frame_offset() == offset {
                match events[event_idx] {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        ..
                    } => {
                        if let Some(free) = self.voices.iter().position(Option::is_none) {
                            self.voices[free] = Some(VoiceSlot {
                                id: occurrence_id,
                                pitch,
                                released: false,
                                release_left: self.cfg.release_frames,
                            });
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        for slot in &mut self.voices {
                            if let Some(v) = slot.as_mut()
                                && v.id == occurrence_id
                                && !v.released
                            {
                                v.released = true;
                                if v.release_left == 0 {
                                    *slot = None;
                                }
                            }
                        }
                    }
                    _ => {}
                }
                event_idx += 1;
            }
            let mut sum = [0.0_f32; 2];
            for slot in &mut self.voices {
                let mut retired = false;
                if let Some(v) = slot.as_mut() {
                    let voice_id = v.id;
                    let pitch = v.pitch;
                    let env = if v.released {
                        if v.release_left == 0 {
                            *slot = None;
                            continue;
                        }
                        let amp = v.release_left as f32 / self.cfg.release_frames.max(1) as f32;
                        v.release_left -= 1;
                        if v.release_left == 0 {
                            retired = true;
                        }
                        amp
                    } else {
                        1.0
                    };
                    let occ_gain = gains
                        .iter()
                        .find(|g| g.occurrence_id() == voice_id)
                        .map_or(1.0_f32, |g| g.value_at_offset(offset as u64) as f32);
                    let base = self.cfg.level * env * occ_gain * (f32::from(pitch) / 100.0);
                    sum[0] += base;
                    sum[1] -= base;
                }
                if retired {
                    *slot = None;
                }
            }
            let delayed = if lat > 0 {
                let d = self.delay[self.next];
                self.delay[self.next] = sum;
                self.next = (self.next + 1) % lat;
                d
            } else {
                sum
            };
            out[0] += delayed[0];
            out[1] += delayed[1];
        }
    }
    fn reset(&mut self) {
        self.voices.fill(None);
        self.delay.fill([0.0; 2]);
        self.next = 0;
    }
    fn is_quiescent(&self) -> bool {
        let lat = self.cfg.latency_frames as usize;
        self.voices.iter().all(Option::is_none)
            && self.delay[..lat].iter().all(|f| f[0] == 0.0 && f[1] == 0.0)
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|o| {
            o.cfg.level.to_bits() == self.cfg.level.to_bits()
                && o.cfg.latency_frames == self.cfg.latency_frames
                && o.cfg.release_frames == self.cfg.release_frames
        })
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(other) = previous.as_any().downcast_ref::<Self>() {
            self.voices = other.voices;
            self.delay = other.delay;
            self.next = other.next;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

fn limits() -> NativeInstrumentRuntimeLimits {
    NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 64,
        max_tail_frames: 48_000,
    }
}

#[test]
fn codec_round_trip_parameters_recursion_prohibition_and_validation() {
    static VOICE_DEF: TestVoiceDef = TestVoiceDef;
    static CHILDREN: [&dyn NativeInstrumentDefinition; 1] = [&VOICE_DEF];
    let child_reg = NativeInstrumentRegistry::new(&CHILDREN).unwrap();
    let rack_def = InstrumentRackDefinition::new(child_reg).unwrap();
    let defs: [&dyn NativeInstrumentDefinition; 1] = [&rack_def];
    let rack_reg = NativeInstrumentRegistry::new(&defs).unwrap();

    let default = rack_reg.default_state(INSTRUMENT_RACK_TYPE_ID).unwrap();
    let patched = rack_reg
        .patch_parameter(&default, PARAM_BRANCH_1_GAIN, ParameterValue::Float(0.75))
        .unwrap();
    let patched = rack_reg
        .patch_parameter(&patched, PARAM_BRANCH_2_BYPASS, ParameterValue::Bool(true))
        .unwrap();
    let decoded = InstrumentRackSpec::decode_state(&patched).unwrap();
    assert_eq!(decoded.branches[0].gain, 0.75);
    assert!(decoded.branches[1].bypassed);
    assert_eq!(decoded.encode_state().unwrap().payload(), patched.payload());

    // Nested rack inside a branch must refuse.
    let nested = InstrumentRackSpec {
        rack_id: id(1),
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(10),
                device_id: id(100),
                gain: 1.0,
                bypassed: false,
                state: Some(default.clone()),
            },
            InstrumentRackBranchSpec {
                branch_id: id(20),
                device_id: id(200),
                gain: 1.0,
                bypassed: false,
                state: None,
            },
        ],
    };
    assert_eq!(
        nested.encode_state().err(),
        Some(DeviceContractError::UnsupportedProcessingClass)
    );

    // Duplicate branch_id or device_id must refuse.
    let dup = InstrumentRackSpec {
        rack_id: id(1),
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(10),
                device_id: id(100),
                gain: 1.0,
                bypassed: false,
                state: None,
            },
            InstrumentRackBranchSpec {
                branch_id: id(10),
                device_id: id(200),
                gain: 1.0,
                bypassed: false,
                state: None,
            },
        ],
    };
    assert_eq!(
        dup.encode_state().err(),
        Some(DeviceContractError::InvalidId)
    );

    // Trailing byte corruption must refuse.
    let mut corrupt = patched.payload().to_vec();
    corrupt.push(0xAA);
    let bad_state = NativeDeviceState::new(
        INSTRUMENT_RACK_TYPE_ID,
        INSTRUMENT_RACK_STATE_VERSION,
        corrupt,
    )
    .unwrap();
    assert_eq!(
        InstrumentRackSpec::decode_state(&bad_state).err(),
        Some(DeviceContractError::MalformedState)
    );
}

#[test]
fn two_branch_asymmetric_latency_sum_matches_independent_oracle_across_partitions_and_gains() {
    static VOICE_DEF: TestVoiceDef = TestVoiceDef;
    static CHILDREN: [&dyn NativeInstrumentDefinition; 1] = [&VOICE_DEF];
    let child_reg = NativeInstrumentRegistry::new(&CHILDREN).unwrap();
    let rack_def = InstrumentRackDefinition::new(child_reg).unwrap();
    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };

    let cfg_a = VoiceConfig {
        level: 0.6,
        latency_frames: 6,
        release_frames: 4,
    };
    let cfg_b = VoiceConfig {
        level: 0.4,
        latency_frames: 2,
        release_frames: 3,
    };
    let state_a = encode_voice(cfg_a);
    let state_b = encode_voice(cfg_b);

    let rack_spec = InstrumentRackSpec {
        rack_id: id(42),
        branches: [
            InstrumentRackBranchSpec {
                branch_id: id(1),
                device_id: id(10),
                gain: 0.5,
                bypassed: false,
                state: Some(state_a.clone()),
            },
            InstrumentRackBranchSpec {
                branch_id: id(2),
                device_id: id(20),
                gain: 0.75,
                bypassed: false,
                state: Some(state_b.clone()),
            },
        ],
    };
    let rack_state = rack_spec.encode_state().unwrap();
    let prepared_rack = rack_def
        .prepare_instrument_with_runtime_limits(&rack_state, &ctx, limits())
        .unwrap();
    assert_eq!(prepared_rack.algorithmic_latency_frames(), Some(6));
    let rack_factory = prepared_rack.prepare_mutable_runtime().unwrap().unwrap();

    // Independent oracle: prepare branch A and branch B separately, delay B by 4 frames, sum weighted.
    let prep_a = VOICE_DEF.prepare_instrument(&state_a, &ctx).unwrap();
    let prep_b = VOICE_DEF.prepare_instrument(&state_b, &ctx).unwrap();
    let mut pair_a = NativeInstrumentRuntimePair::new(
        prep_a.prepare_mutable_runtime().unwrap().unwrap().as_ref(),
        limits(),
    )
    .unwrap();
    let mut pair_b = NativeInstrumentRuntimePair::new(
        prep_b.prepare_mutable_runtime().unwrap().unwrap().as_ref(),
        limits(),
    )
    .unwrap();

    let total = 320_usize; // > INSTRUMENT_RACK_CHUNK_FRAMES (256) to exercise multi-chunk slicing!
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 2,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 10,
            occurrence_id: 2,
            pitch: 64,
            velocity: 90,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 270,
            occurrence_id: 1,
        },
        NativeInstrumentEvent::NoteOff {
            frame_offset: 280,
            occurrence_id: 2,
        },
    ];
    let gains = [NativeInstrumentOccurrenceGain::new(1, 1.0, 0.5, total as u64, 0).unwrap()];

    let mut out_a = vec![[0.0_f32; 2]; total];
    let mut out_b = vec![[0.0_f32; 2]; total];
    pair_a.begin_block();
    pair_a
        .process_block_with_clock_and_occurrence_gains(
            &events,
            &mut out_a,
            None,
            &gains,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    pair_a.commit_block();
    pair_b.begin_block();
    pair_b
        .process_block_with_clock_and_occurrence_gains(
            &events,
            &mut out_b,
            None,
            &gains,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    pair_b.commit_block();

    // Delay B by 4 frames (6 - 2) and sum 0.5*A + 0.75*B_delayed.
    let mut expected = vec![[0.0_f32; 2]; total];
    for i in 0..total {
        let b_sample = if i >= 4 { out_b[i - 4] } else { [0.0; 2] };
        expected[i] = [
            out_a[i][0] * 0.5 + b_sample[0] * 0.75,
            out_a[i][1] * 0.5 + b_sample[1] * 0.75,
        ];
    }

    // Whole-block render (> 256 frames) matches exact oracle.
    let mut rack_whole = NativeInstrumentRuntimePair::new(rack_factory.as_ref(), limits()).unwrap();
    let mut actual_whole = vec![[0.0_f32; 2]; total];
    rack_whole.begin_block();
    rack_whole
        .process_block_with_clock_and_occurrence_gains(
            &events,
            &mut actual_whole,
            None,
            &gains,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    rack_whole.commit_block();
    assert_eq!(actual_whole, expected);
    assert!(rack_whole.committed_is_quiescent());

    // Irregular block partitions match exact oracle bit-for-bit.
    let mut rack_chunked =
        NativeInstrumentRuntimePair::new(rack_factory.as_ref(), limits()).unwrap();
    let mut actual_chunked = vec![[0.0_f32; 2]; total];
    let pattern = [1, 3, 7, 13, 29, 64];
    let mut cursor = 0_usize;
    let mut step = 0_usize;
    while cursor < total {
        let len = pattern[step % pattern.len()].min(total - cursor);
        let end = cursor + len;
        let block_events: Vec<_> = events
            .iter()
            .copied()
            .filter(|e| e.frame_offset() >= cursor && e.frame_offset() < end)
            .map(|e| shift_event_offset(e, e.frame_offset() - cursor))
            .collect();
        let block_gains = [gains[0].advance_frames(cursor as u64).unwrap()];
        rack_chunked.begin_block();
        rack_chunked
            .process_block_with_clock_and_occurrence_gains(
                &block_events,
                &mut actual_chunked[cursor..end],
                None,
                &block_gains,
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        rack_chunked.commit_block();
        cursor = end;
        step += 1;
    }
    assert_eq!(actual_chunked, expected);
    assert!(rack_chunked.committed_is_quiescent());

    // Bypass branch 1 & remove branch 0 -> exact single-branch & empty-rack quiescence + Stop reset.
    let mut active_rack =
        NativeInstrumentRuntimePair::new(rack_factory.as_ref(), limits()).unwrap();
    active_rack.begin_block();
    let mut tmp = vec![[0.0_f32; 2]; 16];
    active_rack
        .process_block(&events[..2], &mut tmp, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    active_rack.commit_block();
    let mut ids = [0_u64; 8];
    let n = active_rack.active_occurrence_ids(&mut ids).unwrap();
    assert_eq!(&ids[..n], &[1, 2]);
    assert!(!active_rack.committed_is_quiescent());

    // Compatible handoff preserves active voices; Reset (Stop) clears both branches cleanly.
    let mut successor = NativeInstrumentRuntimePair::new(rack_factory.as_ref(), limits()).unwrap();
    assert!(successor.can_inherit_from(&active_rack));
    successor.inherit_from(&active_rack).unwrap();
    let n2 = successor.active_occurrence_ids(&mut ids).unwrap();
    assert_eq!(&ids[..n2], &[1, 2]);
    successor.reset();
    assert!(successor.committed_is_quiescent());
}
