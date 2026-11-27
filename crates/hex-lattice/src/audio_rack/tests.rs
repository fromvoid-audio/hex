use super::*;
use crate::{
    DevicePreparationContext, NativeDeviceDefinition, NativeProcessorRuntime, OutputTail,
    PreparedNativeProcessor, StereoMatrix,
};
use hex_lattice_contract::{
    NativeDeviceMetadata, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::any::Any;

const LATENCY_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.latency");
const GAIN_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.gain");
const CLIP_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.clip");
const UNKNOWN_LATENCY_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.unknown-latency");

const PARAM_VALUE: ParameterId = ParameterId::from_static(1);

const LATENCY_DESCRIPTORS: [ParameterDescriptor; 1] = [ParameterDescriptor {
    id: PARAM_VALUE,
    label: "latency",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 256.0,
        unit: Some("frames"),
    },
    default: ParameterValue::Float(4.0),
    presentation: ParameterPresentation::Linear,
}];

const GAIN_DESCRIPTORS: [ParameterDescriptor; 1] = [ParameterDescriptor {
    id: PARAM_VALUE,
    label: "gain",
    kind: ParameterKind::Float {
        min: -4.0,
        max: 4.0,
        unit: None,
    },
    default: ParameterValue::Float(1.0),
    presentation: ParameterPresentation::Linear,
}];

const CLIP_DESCRIPTORS: [ParameterDescriptor; 1] = [ParameterDescriptor {
    id: PARAM_VALUE,
    label: "ceiling",
    kind: ParameterKind::Float {
        min: 0.05,
        max: 2.0,
        unit: None,
    },
    default: ParameterValue::Float(0.5),
    presentation: ParameterPresentation::Linear,
}];

struct LatencyDef {
    declare_unknown: bool,
}

fn latency_state(type_id: DeviceTypeId, frames: u32) -> NativeDeviceState {
    NativeDeviceState::new(type_id, 1, frames.to_le_bytes().to_vec()).unwrap()
}

fn decode_u32(state: &NativeDeviceState) -> Result<u32, DeviceContractError> {
    let bytes: [u8; 4] = state
        .payload()
        .try_into()
        .map_err(|_| DeviceContractError::MalformedState)?;
    let frames = u32::from_le_bytes(bytes);
    if frames > 256 {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(frames)
}

impl NativeDeviceMetadata for LatencyDef {
    fn type_id(&self) -> DeviceTypeId {
        if self.declare_unknown {
            UNKNOWN_LATENCY_TYPE
        } else {
            LATENCY_TYPE
        }
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &LATENCY_DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(latency_state(NativeDeviceMetadata::type_id(self), 4))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let frames = decode_u32(state)?;
        Ok(latency_state(NativeDeviceMetadata::type_id(self), frames))
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if parameter != PARAM_VALUE {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(ParameterValue::Float(decode_u32(state)? as f32))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = decode_u32(state)?;
        match (parameter, value) {
            (PARAM_VALUE, ParameterValue::Float(v))
                if v.is_finite() && (0.0..=256.0).contains(&v) =>
            {
                Ok(latency_state(
                    NativeDeviceMetadata::type_id(self),
                    v.round() as u32,
                ))
            }
            (PARAM_VALUE, _) => Err(DeviceContractError::InvalidParameterValue),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
}

struct PreparedLatency {
    frames: u32,
    declare_unknown: bool,
}

impl PreparedNativeProcessor for PreparedLatency {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        (!self.declare_unknown).then_some(self.frames)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.frames),
        }
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<LatencyRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(LatencyRuntime {
            frames: self.frames as usize,
            ring: [[0.0; 2]; 256],
            next: 0,
        })
    }
}

struct LatencyRuntime {
    frames: usize,
    ring: [[f32; 2]; 256],
    next: usize,
}

impl NativeProcessorRuntime for LatencyRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        if self.frames == 0 {
            return;
        }
        for sample in frames {
            let out = self.ring[self.next];
            self.ring[self.next] = *sample;
            self.next = (self.next + 1) % self.frames;
            *sample = out;
        }
    }
    fn reset(&mut self) {
        self.ring[..self.frames].fill([0.0; 2]);
        self.next = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.ring[..self.frames]
            .iter()
            .all(|f| f[0] == 0.0 && f[1] == 0.0)
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| other.frames == self.frames)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(other) = previous.as_any().downcast_ref::<Self>()
            && other.frames == self.frames
        {
            self.ring = other.ring;
            self.next = other.next;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl NativeDeviceDefinition for LatencyDef {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let frames = decode_u32(state)?;
        Ok(Some(Box::new(PreparedLatency {
            frames,
            declare_unknown: self.declare_unknown,
        })))
    }
}

struct GainDef;

fn float_state(type_id: DeviceTypeId, value: f32) -> NativeDeviceState {
    NativeDeviceState::new(type_id, 1, value.to_le_bytes().to_vec()).unwrap()
}

fn decode_f32(state: &NativeDeviceState, min: f32, max: f32) -> Result<f32, DeviceContractError> {
    let bytes: [u8; 4] = state
        .payload()
        .try_into()
        .map_err(|_| DeviceContractError::MalformedState)?;
    let value = f32::from_le_bytes(bytes);
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(value)
}

impl NativeDeviceMetadata for GainDef {
    fn type_id(&self) -> DeviceTypeId {
        GAIN_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &GAIN_DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(float_state(GAIN_TYPE, 1.0))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let v = decode_f32(state, -4.0, 4.0)?;
        Ok(float_state(GAIN_TYPE, v))
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if parameter != PARAM_VALUE {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(ParameterValue::Float(decode_f32(state, -4.0, 4.0)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = decode_f32(state, -4.0, 4.0)?;
        match (parameter, value) {
            (PARAM_VALUE, ParameterValue::Float(v))
                if v.is_finite() && (-4.0..=4.0).contains(&v) =>
            {
                Ok(float_state(GAIN_TYPE, v))
            }
            (PARAM_VALUE, _) => Err(DeviceContractError::InvalidParameterValue),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
}

impl NativeDeviceDefinition for GainDef {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let g = decode_f32(state, -4.0, 4.0)?;
        StereoMatrix::new([[g, 0.0], [0.0, g]])
            .map_err(|_| DeviceContractError::InvalidParameterValue)
    }
}

struct ClipDef;

impl NativeDeviceMetadata for ClipDef {
    fn type_id(&self) -> DeviceTypeId {
        CLIP_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &CLIP_DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(float_state(CLIP_TYPE, 0.5))
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let v = decode_f32(state, 0.05, 2.0)?;
        Ok(float_state(CLIP_TYPE, v))
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if parameter != PARAM_VALUE {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(ParameterValue::Float(decode_f32(state, 0.05, 2.0)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = decode_f32(state, 0.05, 2.0)?;
        match (parameter, value) {
            (PARAM_VALUE, ParameterValue::Float(v))
                if v.is_finite() && (0.05..=2.0).contains(&v) =>
            {
                Ok(float_state(CLIP_TYPE, v))
            }
            (PARAM_VALUE, _) => Err(DeviceContractError::InvalidParameterValue),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
}

struct PreparedClip(f32);

impl PreparedNativeProcessor for PreparedClip {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<ClipRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(ClipRuntime(self.0))
    }
}

struct ClipRuntime(f32);

impl NativeProcessorRuntime for ClipRuntime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        let limit = self.0;
        for sample in frames {
            sample[0] = sample[0].clamp(-limit, limit);
            sample[1] = sample[1].clamp(-limit, limit);
        }
    }
    fn reset(&mut self) {}
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| other.0.to_bits() == self.0.to_bits())
    }
    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl NativeDeviceDefinition for ClipDef {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let ceiling = decode_f32(state, 0.05, 2.0)?;
        Ok(Some(Box::new(PreparedClip(ceiling))))
    }
}

static LATENCY_DEF: LatencyDef = LatencyDef {
    declare_unknown: false,
};
static UNKNOWN_LATENCY_DEF: LatencyDef = LatencyDef {
    declare_unknown: true,
};
static GAIN_DEF: GainDef = GainDef;
static CLIP_DEF: ClipDef = ClipDef;
static DEFS: [&dyn NativeDeviceDefinition; 4] =
    [&LATENCY_DEF, &UNKNOWN_LATENCY_DEF, &GAIN_DEF, &CLIP_DEF];

fn test_registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFS).unwrap()
}

fn id(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v).unwrap()
}

fn render_all(
    rack: &mut AudioRackRuntime,
    input: &[StereoFrame],
    partitions: &[usize],
) -> Vec<StereoFrame> {
    let mut out = input.to_vec();
    let mut cursor = 0_usize;
    let mut step = 0_usize;
    while cursor < out.len() {
        let len = partitions[step % partitions.len()].min(out.len() - cursor);
        rack.begin_block();
        rack.process_block(&mut out[cursor..cursor + len]).unwrap();
        rack.commit_block();
        cursor += len;
        step += 1;
    }
    out
}

#[test]
fn asymmetric_latency_two_branch_and_empty_branch_impulse_oracles_align_exactly() {
    let registry = test_registry();
    let limits = AudioRackLimits::default();

    // Branch 0: 12-frame latency + gain 0.5; Branch 1: 4-frame latency + gain 0.25.
    // Rack latency = 12 frames; Branch 1 receives exact 8-frame compensation delay.
    let spec = AudioRackSpec {
        rack_id: id(10),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(100),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(1),
                        state: latency_state(LATENCY_TYPE, 12),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(2),
                        state: float_state(GAIN_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(200),
                gain: 0.5,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(3),
                        state: latency_state(LATENCY_TYPE, 4),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(4),
                        state: float_state(GAIN_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    let mut whole = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &spec,
        &registry,
        limits,
    )
    .unwrap();
    assert_eq!(whole.rack_latency_frames(), 12);
    assert_eq!(whole.branch_latency_frames(0), Some(12));
    assert_eq!(whole.branch_latency_frames(1), Some(4));
    assert_eq!(whole.branch_compensation_delay_frames(0), Some(0));
    assert_eq!(whole.branch_compensation_delay_frames(1), Some(8));

    let mut impulse = vec![[0.0_f32; 2]; 32];
    impulse[0] = [1.0, -1.0];
    let expected_whole = render_all(&mut whole, &impulse, &[32]);

    // Impulse must arrive in exact phase alignment at frame 12 with amplitude 0.5 + 0.25 = 0.75.
    for (index, &frame) in expected_whole.iter().enumerate() {
        if index == 12 {
            assert_eq!(frame, [0.75, -0.75]);
        } else {
            assert_eq!(frame, [0.0, 0.0], "unexpected energy at frame {index}");
        }
    }
    assert!(whole.committed_is_quiescent());

    // Irregular block partitions must reproduce the whole-block oracle bit-for-bit.
    let mut chunked = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &spec,
        &registry,
        limits,
    )
    .unwrap();
    let actual_chunked = render_all(&mut chunked, &impulse, &[1, 3, 7, 13, 8]);
    assert_eq!(actual_chunked, expected_whole);
    assert!(chunked.committed_is_quiescent());

    // Empty branch oracle: Branch 0 has 10-frame delay (gain 0.4), Branch 1 is empty dry (gain 0.6).
    let empty_branch_spec = AudioRackSpec {
        rack_id: id(11),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(101),
                gain: 0.4,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(10),
                    state: latency_state(LATENCY_TYPE, 10),
                    bypassed: false,
                }],
            },
            AudioRackBranchSpec {
                branch_id: id(201),
                gain: 0.6,
                muted: false,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    let mut wet_dry = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &empty_branch_spec,
        &registry,
        limits,
    )
    .unwrap();
    assert_eq!(wet_dry.rack_latency_frames(), 10);
    assert_eq!(wet_dry.branch_compensation_delay_frames(1), Some(10));
    let wet_dry_out = render_all(&mut wet_dry, &impulse, &[5, 7, 20]);
    for (index, &frame) in wet_dry_out.iter().enumerate() {
        if index == 10 {
            assert!((frame[0] - 1.0).abs() < 1e-6);
            assert!((frame[1] + 1.0).abs() < 1e-6);
        } else {
            assert_eq!(frame, [0.0, 0.0]);
        }
    }
}

#[test]
fn child_reorder_remove_bypass_history_retained_tails_and_stop_preserve_contracts() {
    let registry = test_registry();
    let limits = AudioRackLimits::default();

    // Branch 0: Gain(2.0) -> Clip(0.5); on input 0.4 -> 0.8 -> clipped to 0.5.
    // Branch 1: Latency(6) (gain 0.0 initially muted so we isolate Branch 0 order first).
    let initial = AudioRackSpec {
        rack_id: id(20),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(100),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(1),
                        state: float_state(GAIN_TYPE, 2.0),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(2),
                        state: float_state(CLIP_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(200),
                gain: 0.5,
                muted: true,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(3),
                    state: latency_state(LATENCY_TYPE, 6),
                    bypassed: false,
                }],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    let mut history = AudioRackHistory::new(initial.clone(), limits).unwrap();
    let mut input = vec![[0.0_f32; 2]; 16];
    input[0] = [0.4, -0.4];

    // Before reorder: Gain(2.0) -> Clip(0.5) => 0.5 at frame 6 (aligned to Branch 1's 6-frame latency).
    let mut r0 = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out0 = render_all(&mut r0, &input, &[16]);
    assert_eq!(out0[6], [0.5, -0.5]);

    // 1. Reorder Branch 0 to Clip(0.5) -> Gain(2.0) => 0.4 clipped to 0.4, *2.0 = 0.8 at frame 6.
    history
        .apply(AudioRackEdit::ReorderDevice {
            branch: 0,
            from: 0,
            to: 1,
        })
        .unwrap();
    let mut r1 = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out1 = render_all(&mut r1, &input, &[16]);
    assert!((out1[6][0] - 0.8).abs() < 1e-6);

    // 2. Child bypass (bypass Gain(2.0) at index 1) => 0.4 at frame 6.
    history
        .apply(AudioRackEdit::SetDeviceBypassed {
            branch: 0,
            index: 1,
            bypassed: true,
        })
        .unwrap();
    let mut r2 = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out2 = render_all(&mut r2, &input, &[16]);
    assert!((out2[6][0] - 0.4).abs() < 1e-6);

    // 3. Remove child (remove Clip at index 0) and bypass rack (latency-aligned dry pass-through).
    history
        .apply(AudioRackEdit::RemoveDevice {
            branch: 0,
            index: 0,
        })
        .unwrap();
    history
        .apply(AudioRackEdit::SetRackBypassed { bypassed: true })
        .unwrap();
    let mut r_byp = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_byp = render_all(&mut r_byp, &input, &[16]);
    assert_eq!(out_byp[6], [0.4, -0.4]);

    // 4. One-step Undo/Redo restores exact prior spec!
    history.undo().unwrap();
    history.undo().unwrap();
    history.undo().unwrap();
    history.undo().unwrap();
    assert_eq!(history.current(), &initial);
    assert!(!history.can_undo());
    history.redo().unwrap();
    assert_eq!(history.current().branches[0].devices[0].device_id, id(2));

    // 5. Retained tail draining + sibling branch inheritance + Stop reset.
    let mut tail_spec = initial.clone();
    tail_spec.branches[1].muted = false;
    let mut active = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &tail_spec,
        &registry,
        limits,
    )
    .unwrap();
    // Feed impulse in 3 frames (not yet emitted at frame 6 -> non-quiescent!).
    let mut prefix = [[0.0_f32; 2]; 3];
    prefix[0] = [0.4, -0.4];
    active.begin_block();
    active.process_block(&mut prefix).unwrap();
    active.commit_block();
    assert!(!active.committed_is_quiescent());

    // Sibling branch inheritance: edit Branch 0 while Branch 1 stays unchanged.
    let mut edited = tail_spec.clone();
    edited.branches[0].devices.clear();
    let mut successor = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &edited,
        &registry,
        limits,
    )
    .unwrap();
    let inherited = successor.inherit_compatible_branches_from(&active).unwrap();
    assert_eq!(inherited, [false, true]);

    // Drain retained tail on predecessor to exact quiescence.
    active.begin_output_tail();
    let mut drain = [[0.0_f32; 2]; 8];
    active.begin_block();
    active.process_block(&mut drain).unwrap();
    active.commit_block();
    // Frame 3 of drain corresponds to global frame 6: 0.5 (branch 0) + 0.5*0.4 (branch 1) = 0.7.
    assert!((drain[3][0] - 0.7).abs() < 1e-6);
    assert!(active.committed_is_quiescent());

    // Stop (reset) clears active delay history immediately.
    successor.reset();
    assert!(successor.committed_is_quiescent());
}

#[test]
fn recursion_duplicate_ids_unknown_latency_budget_and_failed_prep_refuse_atomically() {
    let registry = test_registry();
    let limits = AudioRackLimits::default();

    let valid = AudioRackSpec {
        rack_id: id(30),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(1),
                gain: 1.0,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(10),
                    state: float_state(GAIN_TYPE, 0.5),
                    bypassed: false,
                }],
            },
            AudioRackBranchSpec {
                branch_id: id(2),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    let mut history = AudioRackHistory::new(valid.clone(), limits).unwrap();

    // 1. Malformed nested Audio Rack state and depth-3 recursion prohibition.
    let malformed_nested = NativeDeviceState::new(AUDIO_RACK_TYPE_ID, 2, vec![]).unwrap();
    assert_eq!(
        history.apply(AudioRackEdit::ReplaceBranchDevices {
            branch: 1,
            devices: vec![AudioRackDeviceSpec {
                device_id: id(11),
                state: malformed_nested,
                bypassed: false,
            }],
        }),
        Err(AudioRackError::InvalidNestedState)
    );
    assert_eq!(history.current(), &valid);
    assert!(!history.can_undo());

    // Explicit depth-1 limit refuses valid depth-2 nested rack.
    let inner_valid = AudioRackSpec {
        rack_id: id(31),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(311),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
            AudioRackBranchSpec {
                branch_id: id(312),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    let mut flat_history = AudioRackHistory::new(
        valid.clone(),
        AudioRackLimits {
            max_depth: 1,
            ..limits
        },
    )
    .unwrap();
    assert_eq!(
        flat_history.apply(AudioRackEdit::ReplaceBranchDevices {
            branch: 1,
            devices: vec![AudioRackDeviceSpec {
                device_id: id(11),
                state: inner_valid.encode_state().unwrap(),
                bypassed: false,
            }],
        }),
        Err(AudioRackError::RecursionProhibited)
    );
    assert_eq!(flat_history.current(), &valid);

    // 2. Duplicate child device ID across branches refuses atomically.
    assert_eq!(
        history.apply(AudioRackEdit::ReplaceBranchDevices {
            branch: 1,
            devices: vec![AudioRackDeviceSpec {
                device_id: id(10),
                state: float_state(GAIN_TYPE, 1.0),
                bypassed: false,
            }],
        }),
        Err(AudioRackError::DuplicateDeviceId)
    );
    assert_eq!(history.current(), &valid);

    // 3. Duplicate branch ID refuses.
    let mut dup_branch = valid.clone();
    dup_branch.branches[1].branch_id = id(1);
    assert_eq!(
        AudioRackRuntime::prepare_on_control(
            ProcessorChainOwner::Track(1),
            &dup_branch,
            &registry,
            limits,
        )
        .err(),
        Some(AudioRackError::DuplicateBranchId)
    );

    // 4. Unknown algorithmic latency refuses.
    let mut unknown_lat = valid.clone();
    unknown_lat.branches[1].devices.push(AudioRackDeviceSpec {
        device_id: id(12),
        state: latency_state(UNKNOWN_LATENCY_TYPE, 8),
        bypassed: false,
    });
    assert_eq!(
        AudioRackRuntime::prepare_on_control(
            ProcessorChainOwner::Track(1),
            &unknown_lat,
            &registry,
            limits,
        )
        .err(),
        Some(AudioRackError::UnknownBranchLatency)
    );

    // 5. Compensation delay ceiling and storage budget refusals.
    let mut high_lat = valid.clone();
    high_lat.branches[0].devices.push(AudioRackDeviceSpec {
        device_id: id(13),
        state: latency_state(LATENCY_TYPE, 32),
        bypassed: false,
    });
    let tight_delay = AudioRackLimits {
        max_compensation_delay_frames: 16,
        ..limits
    };
    assert_eq!(
        AudioRackRuntime::prepare_on_control(
            ProcessorChainOwner::Track(1),
            &high_lat,
            &registry,
            tight_delay,
        )
        .err(),
        Some(AudioRackError::CompensationDelayExceeded)
    );
    let tight_bytes = AudioRackLimits {
        max_pair_bytes: 128,
        ..limits
    };
    assert_eq!(
        AudioRackRuntime::prepare_on_control(
            ProcessorChainOwner::Track(1),
            &valid,
            &registry,
            tight_bytes,
        )
        .err(),
        Some(AudioRackError::RuntimeBudgetExceeded)
    );

    // 6. BlockNotStarted and BlockTooLarge refuse cleanly before mutating audio.
    let mut rack = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &valid,
        &registry,
        AudioRackLimits {
            max_block_frames: 16,
            ..limits
        },
    )
    .unwrap();
    let mut sample = [[0.25_f32; 2]; 4];
    assert_eq!(
        rack.process_block(&mut sample),
        Err(AudioRackError::BlockNotStarted)
    );
    assert_eq!(sample[0], [0.25, 0.25]);
    rack.begin_block();
    let mut oversized = [[0.25_f32; 2]; 32];
    assert_eq!(
        rack.process_block(&mut oversized),
        Err(AudioRackError::BlockTooLarge)
    );
    rack.rollback_block();
}

#[test]
fn nested_serial_parallel_impulse_pcm_and_pdc_oracles_align_exactly_across_partitions() {
    let registry = test_registry();
    let limits = AudioRackLimits::default();

    // Inner depth-2 Audio Rack:
    //   Inner Branch 0: Latency(10) -> Gain(0.5)   (branch latency = 10)
    //   Inner Branch 1: Latency(2)  -> Gain(0.25)  (branch latency = 2, comp = 8)
    //   Inner Rack Latency = 10; sum amplitude = 0.5 + 0.25 = 0.75 at +10 frames.
    let inner_spec = AudioRackSpec {
        rack_id: id(200),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(201),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(21),
                        state: latency_state(LATENCY_TYPE, 10),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(22),
                        state: float_state(GAIN_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(202),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(23),
                        state: latency_state(LATENCY_TYPE, 2),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(24),
                        state: float_state(GAIN_TYPE, 0.25),
                        bypassed: false,
                    },
                ],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    // Outer depth-1 Audio Rack:
    //   Outer Branch 0: Latency(4) -> NestedRack(lat 10, amp 0.75) -> Gain(0.5) => lat 14, amp 0.375
    //   Outer Branch 1: Latency(6) -> Gain(0.25)                                => lat 6, comp 8, amp 0.25
    //   Total Outer Rack Latency = 14; combined impulse amplitude at frame 14 = 0.375 + 0.25 = 0.625!
    let outer_spec = AudioRackSpec {
        rack_id: id(100),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(101),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(11),
                        state: latency_state(LATENCY_TYPE, 4),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(12),
                        state: inner_spec.encode_state().unwrap(),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(13),
                        state: float_state(GAIN_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(102),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(14),
                        state: latency_state(LATENCY_TYPE, 6),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(15),
                        state: float_state(GAIN_TYPE, 0.25),
                        bypassed: false,
                    },
                ],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    let mut whole = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &outer_spec,
        &registry,
        limits,
    )
    .unwrap();
    assert_eq!(whole.nested_rack_count(), 1);
    assert_eq!(whole.nested_rack_latency_frames(id(200)), Some(10));
    assert_eq!(whole.rack_latency_frames(), 14);
    assert_eq!(whole.branch_latency_frames(0), Some(14));
    assert_eq!(whole.branch_latency_frames(1), Some(6));
    assert_eq!(whole.branch_compensation_delay_frames(0), Some(0));
    assert_eq!(whole.branch_compensation_delay_frames(1), Some(8));

    let mut impulse = vec![[0.0_f32; 2]; 32];
    impulse[0] = [1.0, -1.0];
    let expected = render_all(&mut whole, &impulse, &[32]);
    for (idx, &frame) in expected.iter().enumerate() {
        if idx == 14 {
            assert!((frame[0] - 0.625).abs() < 1e-6);
            assert!((frame[1] + 0.625).abs() < 1e-6);
        } else {
            assert_eq!(frame, [0.0, 0.0], "unexpected energy at frame {idx}");
        }
    }
    assert!(whole.committed_is_quiescent());

    // Irregular partitions reproduce the nested oracle bit-for-bit.
    let mut chunked = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &outer_spec,
        &registry,
        limits,
    )
    .unwrap();
    let actual_chunked = render_all(&mut chunked, &impulse, &[1, 3, 5, 11, 12]);
    assert_eq!(actual_chunked, expected);

    // Bypassing the nested rack preserves exact 14-frame PDC alignment:
    // Outer Branch 0 dry path = 1.0 * 0.5 = 0.5, Outer Branch 1 = 0.25 -> total 0.75 at frame 14!
    let mut nested_bypassed = outer_spec.clone();
    nested_bypassed.branches[0].devices[1].bypassed = true;
    let mut byp_rt = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        &nested_bypassed,
        &registry,
        limits,
    )
    .unwrap();
    assert_eq!(byp_rt.rack_latency_frames(), 14);
    let byp_out = render_all(&mut byp_rt, &impulse, &[7, 9, 16]);
    for (idx, &frame) in byp_out.iter().enumerate() {
        if idx == 14 {
            assert!((frame[0] - 0.75).abs() < 1e-6);
            assert!((frame[1] + 0.75).abs() < 1e-6);
        } else {
            assert_eq!(frame, [0.0, 0.0]);
        }
    }
}

#[test]
fn nested_group_ungroup_stable_id_edits_cycle_depth_refusal_and_v1_migration() {
    let registry = test_registry();
    let limits = AudioRackLimits::default();

    // Start flat: Branch 0 = [Gain(2.0), Clip(0.5)], Branch 1 = [Latency(6)].
    let flat = AudioRackSpec {
        rack_id: id(50),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(501),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(51),
                        state: float_state(GAIN_TYPE, 2.0),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(52),
                        state: float_state(CLIP_TYPE, 0.5),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(502),
                gain: 0.5,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(53),
                    state: latency_state(LATENCY_TYPE, 6),
                    bypassed: false,
                }],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    // Strict v1 flat -> v2 migration round-trips identically; unsupported v0/v3 refuse!
    let v1_state = flat.encode_v1_flat_state().unwrap();
    assert_eq!(v1_state.state_version(), AUDIO_RACK_STATE_VERSION_V1_FLAT);
    assert_eq!(
        AudioRackSpec::decode_and_migrate_state(&v1_state).unwrap(),
        flat
    );
    assert_eq!(
        AudioRackSpec::decode_state(&v1_state),
        Err(AudioRackError::InvalidNestedState)
    );
    let bad_v5 =
        NativeDeviceState::new(AUDIO_RACK_TYPE_ID, 5, v1_state.payload().to_vec()).unwrap();
    assert_eq!(
        AudioRackSpec::decode_and_migrate_state(&bad_v5),
        Err(AudioRackError::InvalidNestedState)
    );

    let mut history = AudioRackHistory::new(flat.clone(), limits).unwrap();

    // 1. Atomic GroupDevices wraps Branch 0 [Gain(2.0), Clip(0.5)] into nested rack 60!
    history
        .apply(AudioRackEdit::GroupDevices {
            branch: 0,
            start: 0,
            end: 2,
            rack_id: id(60),
            host_device_id: id(59),
            branch_ids: [id(601), id(602)],
        })
        .unwrap();
    assert_eq!(history.current().branches[0].devices.len(), 1);
    assert_eq!(
        history.current().branches[0].devices[0].state.type_id(),
        AUDIO_RACK_TYPE_ID
    );

    // 2. Strict v2 roundtrip preserves exact depth-2 tree!
    let v2_encoded = history.current().encode_state().unwrap();
    assert_eq!(v2_encoded.state_version(), AUDIO_RACK_STATE_VERSION);
    assert_eq!(
        AudioRackSpec::decode_state(&v2_encoded).unwrap(),
        *history.current()
    );
    // v1 flat encoder explicitly refuses nested rack specs.
    assert_eq!(
        history.current().encode_v1_flat_state(),
        Err(AudioRackError::RecursionProhibited)
    );

    // 3. Edit nested rack by stable rack_id (60): reorder Gain(2.0) & Clip(0.5)!
    history
        .apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(60),
            edit: Box::new(AudioRackEdit::ReorderDevice {
                branch: 0,
                from: 0,
                to: 1,
            }),
        })
        .unwrap();
    // Stale/missing nested rack ID refuses atomically without mutating state or undo!
    let before_stale = history.current().clone();
    assert_eq!(
        history.apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(999),
            edit: Box::new(AudioRackEdit::SetRackBypassed { bypassed: true }),
        }),
        Err(AudioRackError::UnknownTargetId)
    );
    assert_eq!(history.current(), &before_stale);

    // 4. Depth-3 nesting refusal (cannot group or nest another rack inside depth-2 rack 60).
    let depth3_candidate = AudioRackSpec {
        rack_id: id(70),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(701),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
            AudioRackBranchSpec {
                branch_id: id(702),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    assert_eq!(
        history.apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(60),
            edit: Box::new(AudioRackEdit::ReplaceBranchDevices {
                branch: 1,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(71),
                    state: depth3_candidate.encode_state().unwrap(),
                    bypassed: false,
                }],
            }),
        }),
        Err(AudioRackError::RecursionProhibited)
    );
    assert_eq!(history.current(), &before_stale);

    // 5. Cycle / duplicate rack_id refusal across levels!
    let mut cycle_spec = flat.clone();
    let self_ref_inner = AudioRackSpec {
        rack_id: id(50), // collides with outer rack_id 50!
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(801),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
            AudioRackBranchSpec {
                branch_id: id(802),
                gain: 1.0,
                muted: false,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    cycle_spec.branches[1].devices.push(AudioRackDeviceSpec {
        device_id: id(80),
        state: self_ref_inner.encode_state().unwrap(),
        bypassed: false,
    });
    assert_eq!(
        cycle_spec.validate(&limits),
        Err(AudioRackError::DuplicateRackId)
    );

    // 6. Verify reordered nested PCM (Clip(0.5) -> Gain(2.0) on 0.4 => 0.8 + 0.5*0.4 = 1.0 at frame 6).
    let mut rt = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let mut inp = vec![[0.0_f32; 2]; 16];
    inp[0] = [0.4, -0.4];
    let out = render_all(&mut rt, &inp, &[16]);
    assert!((out[6][0] - 1.0).abs() < 1e-6);

    // 7. Atomic Ungroup restores flat Branch 0 with reordered stable IDs [52, 51];
    //    3 Undos return to exact initial flat spec!
    history
        .apply(AudioRackEdit::UngroupNestedRack {
            branch: 0,
            index: 0,
            keep_branch: 0,
        })
        .unwrap();
    assert_eq!(history.current().branches[0].devices.len(), 2);
    assert_eq!(history.current().branches[0].devices[0].device_id, id(52));
    assert_eq!(history.current().branches[0].devices[1].device_id, id(51));

    history.undo().unwrap();
    history.undo().unwrap();
    history.undo().unwrap();
    assert_eq!(history.current(), &flat);
    assert!(!history.can_undo());
    history.redo().unwrap();
    assert_eq!(
        history.current().branches[0].devices[0].state.type_id(),
        AUDIO_RACK_TYPE_ID
    );
}

const RESTRICTED_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.rack.restricted");
const PARAM_READONLY: ParameterId = ParameterId::from_static(10);
const PARAM_STRUCTURAL: ParameterId = ParameterId::from_static(11);

const RESTRICTED_DESCRIPTORS: [ParameterDescriptor; 2] = [
    ParameterDescriptor {
        id: PARAM_READONLY,
        label: "meter",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_STRUCTURAL,
        label: "mode",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
];

struct RestrictedDef;

impl NativeDeviceMetadata for RestrictedDef {
    fn type_id(&self) -> DeviceTypeId {
        RESTRICTED_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &RESTRICTED_DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(RESTRICTED_TYPE, 1, vec![0])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != RESTRICTED_TYPE || state.payload().len() != 1 {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let _ = self.migrate_state(state)?;
        match parameter {
            PARAM_READONLY => Ok(ParameterValue::Float(0.0)),
            PARAM_STRUCTURAL => Ok(ParameterValue::Bool(state.payload()[0] != 0)),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = self.migrate_state(state)?;
        match (parameter, value) {
            (PARAM_STRUCTURAL, ParameterValue::Bool(b)) => {
                NativeDeviceState::new(RESTRICTED_TYPE, 1, vec![u8::from(b)])
            }
            (PARAM_READONLY, _) => Err(DeviceContractError::InvalidParameterValue),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
    fn parameter_semantics_for_state(
        &self,
        _state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterModulation,
            ParameterNormalization, ParameterSemantics, ParameterSmoothing, ParameterStepping,
        };
        match parameter {
            PARAM_READONLY => Ok(Some(ParameterSemantics {
                access: ParameterAccess::ReadOnly,
                normalization: ParameterNormalization::Linear,
                stepping: ParameterStepping::Continuous,
                automation: ParameterAutomation::Unsupported,
                modulation: ParameterModulation::Unsupported,
                smoothing: ParameterSmoothing::None,
                change_impact: ParameterChangeImpact::ValueOnly,
            })),
            PARAM_STRUCTURAL => Ok(Some(ParameterSemantics {
                access: ParameterAccess::ReadWrite,
                normalization: ParameterNormalization::Discrete,
                stepping: ParameterStepping::Boolean,
                automation: ParameterAutomation::Unsupported,
                modulation: ParameterModulation::Unsupported,
                smoothing: ParameterSmoothing::None,
                change_impact: ParameterChangeImpact::Structural,
            })),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }
}

impl NativeDeviceDefinition for RestrictedDef {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
}

#[test]
fn macro_multi_target_ranges_polarity_precedence_reorder_stale_structural_refusal_and_v2_migration()
{
    static RESTRICTED_DEF: RestrictedDef = RestrictedDef;
    static EXT_DEFS: [&dyn NativeDeviceDefinition; 5] = [
        &LATENCY_DEF,
        &UNKNOWN_LATENCY_DEF,
        &GAIN_DEF,
        &CLIP_DEF,
        &RESTRICTED_DEF,
    ];
    let registry = NativeDeviceRegistry::new(&EXT_DEFS).unwrap();
    let limits = AudioRackLimits::default();

    // Depth-2 nested rack:
    //   Inner Branch 0: Gain(authored 1.0, dev 21) -> Clip(authored 1.0, dev 22)
    //   Inner Branch 1: RestrictedDef (dev 23, for read-only / structural refusal tests)
    let inner = AudioRackSpec {
        rack_id: id(200),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(201),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(21),
                        state: float_state(GAIN_TYPE, 1.0),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(22),
                        state: float_state(CLIP_TYPE, 1.0),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(202),
                gain: 0.0,
                muted: true,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(23),
                    state: RESTRICTED_DEF.default_state().unwrap(),
                    bypassed: false,
                }],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    let base_outer = AudioRackSpec {
        rack_id: id(100),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(101),
                gain: 1.0,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(11),
                    state: inner.encode_state().unwrap(),
                    bypassed: false,
                }],
            },
            AudioRackBranchSpec {
                branch_id: id(102),
                gain: 0.0,
                muted: true,
                devices: Vec::new(),
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };

    // Prior v2 nested state (no macros) migrates cleanly to v3!
    let v2_state = base_outer.encode_v2_nested_state().unwrap();
    assert_eq!(v2_state.state_version(), AUDIO_RACK_STATE_VERSION_V2_NESTED);
    assert_eq!(
        AudioRackSpec::decode_and_migrate_state(&v2_state).unwrap(),
        base_outer
    );

    let mut history = AudioRackHistory::new(base_outer.clone(), limits).unwrap();
    history
        .apply(AudioRackEdit::AddMacro {
            macro_id: id(900),
            value: 0.5,
        })
        .unwrap();

    // Map macro 900 to BOTH nested devices:
    //   1) Gain (dev 21, domain -4..4): norm [0.5, 1.0] Normal => at macro=0.5 -> norm 0.75 -> gain = 2.0!
    //   2) Clip (dev 22, domain 0.05..2.0): norm [0.0, 0.5] Inverted => at macro=0.5 -> norm 0.25 -> ceiling = 0.5375!
    let gain_target = AudioRackMacroTarget {
        rack_id: id(200),
        device_id: id(21),
        parameter_id: PARAM_VALUE,
    };
    let clip_target = AudioRackMacroTarget {
        rack_id: id(200),
        device_id: id(22),
        parameter_id: PARAM_VALUE,
    };
    history
        .apply(AudioRackEdit::AddMacroMapping {
            macro_id: id(900),
            mapping: AudioRackMacroMapping {
                target: gain_target,
                min_normalized: 0.5,
                max_normalized: 1.0,
                polarity: AudioRackMacroPolarity::Normal,
                precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
                bypassed: false,
            },
        })
        .unwrap();
    history
        .apply(AudioRackEdit::AddMacroMapping {
            macro_id: id(900),
            mapping: AudioRackMacroMapping {
                target: clip_target,
                min_normalized: 0.0,
                max_normalized: 0.5,
                polarity: AudioRackMacroPolarity::Inverted,
                precedence: AudioRackMacroPrecedence::AutomationOverridesMacro,
                bypassed: false,
            },
        })
        .unwrap();

    // Strict v3 roundtrip preserves exact macro value + both multi-target mappings!
    let v3_encoded = history.current().encode_state().unwrap();
    assert_eq!(v3_encoded.state_version(), AUDIO_RACK_STATE_VERSION);
    assert_eq!(
        AudioRackSpec::decode_state(&v3_encoded).unwrap(),
        *history.current()
    );

    // 1. PCM oracle at macro=0.5: input 0.4 -> Gain(2.0) -> 0.8 -> Clip(0.5375) -> 0.5375!
    let mut rt = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out = render_all(&mut rt, &[[0.4, -0.4]; 4], &[4]);
    assert!((out[0][0] - 0.5375).abs() < 1e-5);
    assert!((out[0][1] + 0.5375).abs() < 1e-5);

    // 2. Automation precedence oracle: when clip_target (dev 22) has active automation,
    //    AutomationOverridesMacro lets authored/automated clip ceiling (1.0) win -> 0.4 * 2.0 = 0.8!
    let mut rt_auto = AudioRackRuntime::prepare_on_control_with_overrides(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
        &[(id(22), PARAM_VALUE)],
    )
    .unwrap();
    let out_auto = render_all(&mut rt_auto, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_auto[0][0] - 0.8).abs() < 1e-5);

    // 3. Macro bypass restores authored Gain(1.0) -> Clip(1.0) => 0.4!
    history
        .apply(AudioRackEdit::SetMacroBypassed {
            macro_id: id(900),
            bypassed: true,
        })
        .unwrap();
    let mut rt_byp = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_byp = render_all(&mut rt_byp, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_byp[0][0] - 0.4).abs() < 1e-5);
    history.undo().unwrap(); // one-step Undo restores active macro!

    // 4. Reordering mapped nested devices keeps stable target tracking:
    //    Clip(0.5375) -> Gain(2.0) on input 0.4 => 0.4 * 2.0 = 0.8!
    history
        .apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(200),
            edit: Box::new(AudioRackEdit::ReorderDevice {
                branch: 0,
                from: 0,
                to: 1,
            }),
        })
        .unwrap();
    let mut rt_reordered = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_reordered = render_all(&mut rt_reordered, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_reordered[0][0] - 0.8).abs() < 1e-5);

    // 5. Deleting a mapped nested device refuses atomically (no silent retargeting!).
    let before_del = history.current().clone();
    assert_eq!(
        history.apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(200),
            edit: Box::new(AudioRackEdit::RemoveDevice {
                branch: 0,
                index: 0,
            }),
        }),
        Err(AudioRackError::InvalidMacroTarget)
    );
    assert_eq!(history.current(), &before_del);

    // 6. Stale/missing target refuses atomically.
    assert_eq!(
        history.apply(AudioRackEdit::AddMacroMapping {
            macro_id: id(900),
            mapping: AudioRackMacroMapping {
                target: AudioRackMacroTarget {
                    rack_id: id(999),
                    device_id: id(21),
                    parameter_id: PARAM_VALUE,
                },
                min_normalized: 0.0,
                max_normalized: 1.0,
                polarity: AudioRackMacroPolarity::Normal,
                precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
                bypassed: false,
            },
        }),
        Err(AudioRackError::InvalidMacroTarget)
    );

    // 7. Read-only and structural parameter targets refuse visibly during control resolution/prepare!
    for bad_param in [
        PARAM_READONLY,
        PARAM_STRUCTURAL,
        ParameterId::from_static(99),
    ] {
        let mut bad_spec = history.current().clone();
        bad_spec.macros[0].mappings.push(AudioRackMacroMapping {
            target: AudioRackMacroTarget {
                rack_id: id(200),
                device_id: id(23),
                parameter_id: bad_param,
            },
            min_normalized: 0.0,
            max_normalized: 1.0,
            polarity: AudioRackMacroPolarity::Normal,
            precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
            bypassed: false,
        });
        assert_eq!(
            AudioRackRuntime::prepare_on_control(
                ProcessorChainOwner::Track(1),
                &bad_spec,
                &registry,
                limits,
            )
            .err(),
            Some(AudioRackError::UnsupportedMacroTarget)
        );
    }

    // 8. Unmapping then deleting succeeds cleanly, and Undo restores the mapping + device!
    history
        .apply(AudioRackEdit::RemoveMacroMapping {
            macro_id: id(900),
            target: clip_target,
        })
        .unwrap();
    history
        .apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(200),
            edit: Box::new(AudioRackEdit::RemoveDevice {
                branch: 0,
                index: 0, // clip is at index 0 after reorder
            }),
        })
        .unwrap();
    history.undo().unwrap();
    history.undo().unwrap();
    assert_eq!(history.current(), &before_del);
    history.redo().unwrap();
    assert_eq!(history.current().macros[0].mappings.len(), 1);
}

#[test]
fn macro_variation_atomic_save_recall_pcm_undo_redo_migration_and_refusals() {
    static RESTRICTED_DEF: RestrictedDef = RestrictedDef;
    static EXT_DEFS: [&dyn NativeDeviceDefinition; 5] = [
        &LATENCY_DEF,
        &UNKNOWN_LATENCY_DEF,
        &GAIN_DEF,
        &CLIP_DEF,
        &RESTRICTED_DEF,
    ];
    let registry = NativeDeviceRegistry::new(&EXT_DEFS).unwrap();
    let limits = AudioRackLimits::default();

    // Depth-2 nested rack with two real mapped targets controlled by one macro:
    //   Gain (dev 21, -4..4) on [0.5, 1.0] Normal
    //   Clip (dev 22, 0.05..2.0) on [0.0, 0.5] Inverted
    let inner = AudioRackSpec {
        rack_id: id(200),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(201),
                gain: 1.0,
                muted: false,
                devices: vec![
                    AudioRackDeviceSpec {
                        device_id: id(21),
                        state: float_state(GAIN_TYPE, 1.0),
                        bypassed: false,
                    },
                    AudioRackDeviceSpec {
                        device_id: id(22),
                        state: float_state(CLIP_TYPE, 1.0),
                        bypassed: false,
                    },
                ],
            },
            AudioRackBranchSpec {
                branch_id: id(202),
                gain: 0.0,
                muted: true,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(23),
                    state: RESTRICTED_DEF.default_state().unwrap(),
                    bypassed: false,
                }],
            },
        ],
        macros: Vec::new(),
        variations: Vec::new(),
    };
    let outer = AudioRackSpec {
        rack_id: id(100),
        sample_rate: 48_000,
        bypassed: false,
        branches: [
            AudioRackBranchSpec {
                branch_id: id(101),
                gain: 1.0,
                muted: false,
                devices: vec![AudioRackDeviceSpec {
                    device_id: id(11),
                    state: inner.encode_state().unwrap(),
                    bypassed: false,
                }],
            },
            AudioRackBranchSpec {
                branch_id: id(102),
                gain: 0.0,
                muted: true,
                devices: Vec::new(),
            },
        ],
        macros: vec![AudioRackMacroSpec {
            macro_id: id(900),
            value: 0.25,
            bypassed: false,
            mappings: vec![
                AudioRackMacroMapping {
                    target: AudioRackMacroTarget {
                        rack_id: id(200),
                        device_id: id(21),
                        parameter_id: PARAM_VALUE,
                    },
                    min_normalized: 0.5,
                    max_normalized: 1.0,
                    polarity: AudioRackMacroPolarity::Normal,
                    precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
                    bypassed: false,
                },
                AudioRackMacroMapping {
                    target: AudioRackMacroTarget {
                        rack_id: id(200),
                        device_id: id(22),
                        parameter_id: PARAM_VALUE,
                    },
                    min_normalized: 0.0,
                    max_normalized: 0.5,
                    polarity: AudioRackMacroPolarity::Inverted,
                    precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
                    bypassed: false,
                },
            ],
        }],
        variations: Vec::new(),
    };

    // Prior v3 macro state migrates cleanly to v4!
    let v3_state = outer.encode_v3_macros_state().unwrap();
    assert_eq!(v3_state.state_version(), AUDIO_RACK_STATE_VERSION_V3_MACROS);
    assert_eq!(
        AudioRackSpec::decode_and_migrate_state(&v3_state).unwrap(),
        outer
    );

    let mut history = AudioRackHistory::new(outer, limits).unwrap();
    // Save variation 1 ("soft") at macro=0.25 -> Gain=1.0, Clip=0.78125 (0.4*1.0 = 0.4)
    history
        .apply(AudioRackEdit::SaveMacroVariation {
            variation_id: id(1001),
            name: "soft".into(),
        })
        .unwrap();
    // Sweep macro to 0.75 and save variation 2 ("hot") -> Gain=3.0, Clip=0.29375 (0.4*3.0=1.2 -> clipped 0.29375)
    history
        .apply(AudioRackEdit::SetMacroValue {
            macro_id: id(900),
            value: 0.75,
        })
        .unwrap();
    history
        .apply(AudioRackEdit::SaveMacroVariation {
            variation_id: id(1002),
            name: "hot".into(),
        })
        .unwrap();

    // Strict v4 roundtrip preserves both named variations!
    let v4_state = history.current().encode_state().unwrap();
    assert_eq!(v4_state.state_version(), AUDIO_RACK_STATE_VERSION);
    assert_eq!(
        AudioRackSpec::decode_state(&v4_state).unwrap(),
        *history.current()
    );

    // PCM at "hot" (macro=0.75): Gain = 3.0, Clip = 0.05 + 0.125 * 1.95 = 0.29375.
    let mut rt_hot = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_hot = render_all(&mut rt_hot, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_hot[0][0] - 0.29375).abs() < 1e-5);

    // One atomic recall of "soft" (id 1001) updates BOTH mapped targets via canonical preparation!
    let ledger = history
        .recall_variation_on_control(id(1001), &registry)
        .unwrap();
    assert_eq!(ledger.len(), 2);
    assert_eq!(history.current().macros[0].value, 0.25);
    let mut rt_soft = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_soft = render_all(&mut rt_soft, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_soft[0][0] - 0.4).abs() < 1e-5);

    // One Undo restores "hot" (0.75), one Redo restores "soft" (0.25)!
    history.undo().unwrap();
    assert_eq!(history.current().macros[0].value, 0.75);
    history.redo().unwrap();
    assert_eq!(history.current().macros[0].value, 0.25);

    // Reordering nested mapped devices keeps variation recall working via stable IDs!
    history
        .apply(AudioRackEdit::EditNestedRackById {
            nested_rack_id: id(200),
            edit: Box::new(AudioRackEdit::ReorderDevice {
                branch: 0,
                from: 0,
                to: 1,
            }),
        })
        .unwrap();
    history
        .recall_variation_on_control(id(1002), &registry)
        .unwrap();
    // Clip(0.29375) -> Gain(3.0) on input 0.4 => 0.29375 * 3.0 = 0.88125!
    let mut rt_reordered_hot = AudioRackRuntime::prepare_on_control(
        ProcessorChainOwner::Track(1),
        history.current(),
        &registry,
        limits,
    )
    .unwrap();
    let out_reordered_hot = render_all(&mut rt_reordered_hot, &[[0.4, -0.4]; 4], &[4]);
    assert!((out_reordered_hot[0][0] - 0.88125).abs() < 1e-5);

    // Refusals leave prior values & history untouched:
    //   1. Stale/unknown variation ID
    //   2. Blank variation name
    //   3. Duplicate variation ID
    //   4. Structural target during recall_variation_on_control
    let snapshot = history.current().clone();
    assert_eq!(
        history.recall_variation_on_control(id(9999), &registry),
        Err(AudioRackError::UnknownTargetId)
    );
    assert_eq!(
        history.apply(AudioRackEdit::SaveMacroVariation {
            variation_id: id(1003),
            name: "   ".into(),
        }),
        Err(AudioRackError::InvalidVariationName)
    );
    assert_eq!(
        history.apply(AudioRackEdit::SaveMacroVariation {
            variation_id: id(1001),
            name: "dup".into(),
        }),
        Err(AudioRackError::DuplicateVariationId)
    );
    assert_eq!(history.current(), &snapshot);

    // Add a structural parameter mapping onto macro 900 -> recall_variation_on_control MUST refuse
    // visibly without changing macro 900's canonical value (remains 0.75)!
    history
        .apply(AudioRackEdit::AddMacroMapping {
            macro_id: id(900),
            mapping: AudioRackMacroMapping {
                target: AudioRackMacroTarget {
                    rack_id: id(200),
                    device_id: id(23),
                    parameter_id: PARAM_STRUCTURAL,
                },
                min_normalized: 0.0,
                max_normalized: 1.0,
                polarity: AudioRackMacroPolarity::Normal,
                precedence: AudioRackMacroPrecedence::MacroOverridesAuthored,
                bypassed: false,
            },
        })
        .unwrap();
    let before_refused_recall = history.current().clone();
    assert_eq!(
        history.recall_variation_on_control(id(1001), &registry),
        Err(AudioRackError::UnsupportedMacroTarget)
    );
    assert_eq!(history.current(), &before_refused_recall);
    assert_eq!(history.current().macros[0].value, 0.75);
}
