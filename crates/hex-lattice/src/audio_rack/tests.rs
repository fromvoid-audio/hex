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
    };
    let mut history = AudioRackHistory::new(valid.clone(), limits).unwrap();

    // 1. Recursion prohibition (nested Audio Rack type inside a branch).
    let nested_state = NativeDeviceState::new(AUDIO_RACK_TYPE_ID, 1, vec![]).unwrap();
    assert_eq!(
        history.apply(AudioRackEdit::ReplaceBranchDevices {
            branch: 1,
            devices: vec![AudioRackDeviceSpec {
                device_id: id(11),
                state: nested_state,
                bypassed: false,
            }],
        }),
        Err(AudioRackError::RecursionProhibited)
    );
    assert_eq!(history.current(), &valid);
    assert!(!history.can_undo());

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
