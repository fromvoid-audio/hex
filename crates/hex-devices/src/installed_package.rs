//! Generic host adapters allowing installed `.hexdevice` C ABI v1 packages
//! (effects and instruments) to execute through the ordinary `hex-lattice` /
//! `hex-engine` registry, preparation, and callback publication paths (`#2992`).
//!
//! # Ownership & Safety Contract
//! - All native `dlopen`/`dlsym`/`dlclose` and C vtable calls stay inside the
//!   excluded `third_party/hex-device-abi` crate; this module contains zero
//!   `unsafe` code.
//! - Loading, descriptor negotiation, and instance creation happen strictly on
//!   control threads (never on Play or audio callbacks).
//! - Each prepared runtime holds an immutable `Arc`-backed [`PackageModule`]
//!   generation lease; destruction and `dlclose` occur only on control once
//!   every live instance has retired.
//! - Audio-callback processing uses fixed preallocated buffers (`512` stereo
//!   frames, `64` note/parameter events, `4096` state transfer bytes) with zero
//!   heap allocations or frees. Any C ABI fault or non-finite output silences
//!   the block rather than replaying stale audio.
//! - Canonical `NativeDeviceState` payloads wrap opaque device state with an
//!   exact [`PackageIdentityEnvelope`] (`publisher_id`, `type_id`, versions,
//!   `package_sha256`); mismatched or unresolved generations refuse
//!   nondestructively without modifying stored state.

use hex_device_abi::{BorrowedAbiPcm, BorrowedAbiResource, PackageInstance, PackageModule};
use hex_lattice::{
    DevicePreparationContext, DeviceResourcePcm, DeviceResources, InstrumentParameterRuntime,
    MutableInstrumentRuntime, NativeDeviceDefinition, NativeInstrumentClock,
    NativeInstrumentDefinition, NativeInstrumentEvent, NativeInstrumentExpression,
    NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError, NativeParameterAutomation,
    OutputTail, OwnedProcessorRuntime, PreparedAnalyticNote, PreparedMutableInstrumentRuntime,
    PreparedNativeInstrument, PreparedOwnedProcessorRuntime, PreparedProcessorFactory, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, AbiResourceDescriptor,
    AbiStatus, CAP_BORROWED_RESOURCES, CAP_CHANNEL_EXPRESSION, CAP_NOTE_EXPRESSION,
    CAP_PARAMETER_EVENTS, CAP_STATE_PERSISTENCE, DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT,
    NOTE_EVENT_ALL_NOTES_OFF, NOTE_EVENT_CC, NOTE_EVENT_OFF, NOTE_EVENT_ON, NOTE_EVENT_PITCH_BEND,
    NOTE_EVENT_PRESSURE, PACKAGE_ABI_MAX_FRAMES, PACKAGE_ABI_MAX_NOTE_EVENTS,
    PACKAGE_ABI_MAX_PARAM_EVENTS, PACKAGE_ABI_MAX_PARAMS, PACKAGE_ABI_MAX_STATE_BYTES,
    PackageIdentityEnvelope, RESOURCE_LAYOUT_MONO, RESOURCE_LAYOUT_STEREO_INTERLEAVED,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_DEVICE_ASSET_REFERENCES, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::any::Any;
use std::path::Path;
use std::sync::Arc;

const MAX_PACKAGE_VOICES: usize = 16;
const PARAM_TABLE_MAGIC: [u8; 4] = *b"HXPM";
const EMPTY_MONO_PCM: &[f32] = &[];

/// Dispatch one C ABI process call using caller-borrowed immutable PCM
/// resources on the stack with zero heap allocations (`#3002`).
fn call_process_with_borrowed_resources(
    inst: &mut PackageInstance,
    supports_borrowed_resources: bool,
    input: &[[f32; 2]],
    output: &mut [[f32; 2]],
    param_events: &[AbiParamEvent],
    note_events: &[AbiNoteEvent],
    resources: &dyn DeviceResources,
) -> Result<(), AbiStatus> {
    if !supports_borrowed_resources {
        return inst.process(input, output, param_events, note_events);
    }
    let count = resources.len();
    if count > MAX_DEVICE_ASSET_REFERENCES {
        return Err(AbiStatus::InvalidArg);
    }
    let mut borrowed = [BorrowedAbiResource {
        meta: AbiResourceDescriptor::empty(),
        pcm: BorrowedAbiPcm::Mono(EMPTY_MONO_PCM),
    }; MAX_DEVICE_ASSET_REFERENCES];
    for (idx, dst) in borrowed.iter_mut().take(count).enumerate() {
        let view = resources.get(idx).ok_or(AbiStatus::InvalidArg)?;
        let (layout, pcm) = match view.pcm {
            DeviceResourcePcm::Mono(slice) => (RESOURCE_LAYOUT_MONO, BorrowedAbiPcm::Mono(slice)),
            DeviceResourcePcm::Stereo(slice) => (
                RESOURCE_LAYOUT_STEREO_INTERLEAVED,
                BorrowedAbiPcm::Stereo(slice),
            ),
        };
        let frame_count = u64::try_from(view.pcm.frames()).map_err(|_| AbiStatus::InvalidArg)?;
        *dst = BorrowedAbiResource {
            meta: AbiResourceDescriptor {
                struct_size: std::mem::size_of::<AbiResourceDescriptor>() as u32,
                slot: view.slot.value(),
                asset_id: view.identity.asset.value(),
                sample_rate: view.sample_rate,
                channel_layout: layout,
                frame_count,
                derivation_version: view.identity.derivation_version,
                reserved: 0,
                content_sha256: view.identity.content,
                derivation_sha256: view.identity.derivation,
            },
            pcm,
        };
    }
    inst.process_with_resources(input, output, param_events, note_events, &borrowed[..count])
}

fn static_param_label(index: usize, raw: &str) -> &'static str {
    match raw {
        "gain" => "gain",
        "balance" => "balance",
        "width" => "width",
        "invert_left" => "invert_left",
        "invert_right" => "invert_right",
        "mute" => "mute",
        "drive" => "drive",
        "mix" => "mix",
        "trim" => "trim",
        "cutoff" => "cutoff",
        "resonance" => "resonance",
        "attack" => "attack",
        "release" => "release",
        "threshold" => "threshold",
        "ratio" => "ratio",
        "rate" => "rate",
        "amount" => "amount",
        "depth" => "depth",
        "feedback" => "feedback",
        "decay" => "decay",
        _ => match index {
            0 => "param_0",
            1 => "param_1",
            2 => "param_2",
            3 => "param_3",
            4 => "param_4",
            5 => "param_5",
            6 => "param_6",
            7 => "param_7",
            8 => "param_8",
            9 => "param_9",
            10 => "param_10",
            11 => "param_11",
            12 => "param_12",
            13 => "param_13",
            14 => "param_14",
            15 => "param_15",
            _ => "param",
        },
    }
}

fn abi_param_to_descriptor(
    index: usize,
    raw: &AbiParameterDescriptor,
) -> Result<(ParameterDescriptor, String), DeviceContractError> {
    let name = raw
        .validate()
        .map_err(|_| DeviceContractError::InvalidDescriptor)?;
    let id = ParameterId::new(raw.id)?;
    let label = static_param_label(index, name);
    let descriptor = ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float {
            min: raw.min,
            max: raw.max,
            unit: None,
        },
        default: ParameterValue::Float(raw.default_value),
        presentation: ParameterPresentation::Linear,
    };
    descriptor.validate_descriptor()?;
    Ok((descriptor, name.to_owned()))
}

fn encode_param_wrapped_state(
    identity: &PackageIdentityEnvelope,
    params: &[(u32, f32)],
    raw_state: &[u8],
) -> Result<Vec<u8>, DeviceContractError> {
    let mut inner = Vec::with_capacity(8 + params.len() * 8 + raw_state.len());
    inner.extend_from_slice(&PARAM_TABLE_MAGIC);
    inner.extend_from_slice(&(params.len() as u16).to_le_bytes());
    inner.extend_from_slice(&(raw_state.len() as u16).to_le_bytes());
    for &(id, val) in params {
        inner.extend_from_slice(&id.to_le_bytes());
        inner.extend_from_slice(&val.to_le_bytes());
    }
    inner.extend_from_slice(raw_state);
    identity.encode_with_raw_state(&inner)
}

type DecodedParamState<'a> = (PackageIdentityEnvelope, Vec<(u32, f32)>, &'a [u8]);
type ValidatedParamState<'a> = (Vec<(u32, f32)>, &'a [u8]);

fn decode_param_wrapped_state(bytes: &[u8]) -> Result<DecodedParamState<'_>, DeviceContractError> {
    let (identity, inner) = PackageIdentityEnvelope::decode_with_raw_state(bytes)?;
    if inner.len() < 8 || inner[0..4] != PARAM_TABLE_MAGIC {
        return Err(DeviceContractError::MalformedState);
    }
    let count = usize::from(u16::from_le_bytes([inner[4], inner[5]]));
    let raw_len = usize::from(u16::from_le_bytes([inner[6], inner[7]]));
    let table_end = 8usize
        .checked_add(
            count
                .checked_mul(8)
                .ok_or(DeviceContractError::MalformedState)?,
        )
        .ok_or(DeviceContractError::MalformedState)?;
    if table_end
        .checked_add(raw_len)
        .ok_or(DeviceContractError::MalformedState)?
        != inner.len()
    {
        return Err(DeviceContractError::MalformedState);
    }
    let mut params = Vec::with_capacity(count);
    let mut cursor = 8usize;
    for _ in 0..count {
        let id = u32::from_le_bytes([
            inner[cursor],
            inner[cursor + 1],
            inner[cursor + 2],
            inner[cursor + 3],
        ]);
        let val = f32::from_le_bytes([
            inner[cursor + 4],
            inner[cursor + 5],
            inner[cursor + 6],
            inner[cursor + 7],
        ]);
        if !val.is_finite() {
            return Err(DeviceContractError::MalformedState);
        }
        params.push((id, val));
        cursor += 8;
    }
    Ok((identity, params, &inner[table_end..]))
}

#[derive(Clone)]
struct SharedPackageDefinition {
    module: PackageModule,
    descriptor: AbiDescriptor,
    type_id: DeviceTypeId,
    identity: PackageIdentityEnvelope,
    parameters: Vec<ParameterDescriptor>,
    parameter_names: Vec<String>,
    default_raw_state: Vec<u8>,
}

impl SharedPackageDefinition {
    fn load(
        dylib_path: &Path,
        publisher_id: String,
        package_sha256: [u8; 32],
        expected_kind: u32,
    ) -> Result<Self, DeviceContractError> {
        let module =
            PackageModule::load(dylib_path).map_err(|_| DeviceContractError::InvalidDescriptor)?;
        let descriptor = module.descriptor();
        let (type_id, versions) = descriptor
            .validate()
            .map_err(|_| DeviceContractError::InvalidDescriptor)?;
        if descriptor.device_kind != expected_kind {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let mut parameters = Vec::with_capacity(descriptor.parameter_count as usize);
        let mut parameter_names = Vec::with_capacity(descriptor.parameter_count as usize);
        for i in 0..descriptor.parameter_count {
            let raw = module
                .parameter_descriptor(i)
                .map_err(|_| DeviceContractError::InvalidDescriptor)?;
            let (desc, name) = abi_param_to_descriptor(i as usize, &raw)?;
            parameters.push(desc);
            parameter_names.push(name);
        }
        hex_lattice_contract::validate_parameter_descriptors(&parameters)?;

        // Capture canonical default raw state once on control.
        let inst = module
            .create_instance(48_000)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        let default_raw_state = if (descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0 {
            let mut buf = vec![0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize];
            let len = inst
                .get_state(&mut buf)
                .map_err(|_| DeviceContractError::MalformedState)?;
            buf.truncate(len);
            buf
        } else {
            Vec::new()
        };

        let type_id_str = type_id.as_str()?.to_owned();
        let identity = PackageIdentityEnvelope {
            publisher_id,
            type_id: type_id_str,
            package_major: versions.package_major,
            package_minor: versions.package_minor,
            package_patch: versions.package_patch,
            state_version: versions.state_version,
            abi_version: versions.abi_version,
            ui_version: versions.ui_version,
            package_sha256,
        };

        Ok(Self {
            module,
            descriptor,
            type_id,
            identity,
            parameters,
            parameter_names,
            default_raw_state,
        })
    }

    fn default_param_pairs(&self) -> Vec<(u32, f32)> {
        self.parameters
            .iter()
            .map(|p| {
                let v = match p.default {
                    ParameterValue::Float(f) => f,
                    ParameterValue::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    ParameterValue::Choice(c) => c as f32,
                };
                (p.id.value(), v)
            })
            .collect()
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        let payload = encode_param_wrapped_state(
            &self.identity,
            &self.default_param_pairs(),
            &self.default_raw_state,
        )?;
        NativeDeviceState::new(self.type_id, self.descriptor.state_version, payload)
    }

    fn validate_and_decode<'a>(
        &self,
        state: &'a NativeDeviceState,
    ) -> Result<ValidatedParamState<'a>, DeviceContractError> {
        if state.type_id() != self.type_id {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != self.descriptor.state_version {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let (identity, params, raw) = decode_param_wrapped_state(state.payload())?;
        // Refuse mismatched package generation nondestructively.
        if identity != self.identity {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if params.len() != self.parameters.len() {
            return Err(DeviceContractError::MalformedState);
        }
        for (&(id, val), desc) in params.iter().zip(self.parameters.iter()) {
            if id != desc.id.value() {
                return Err(DeviceContractError::MalformedState);
            }
            desc.validate(ParameterValue::Float(val))?;
        }
        Ok((params, raw))
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let (params, _) = self.validate_and_decode(state)?;
        let (_, val) = params
            .into_iter()
            .find(|&(id, _)| id == parameter.value())
            .ok_or(DeviceContractError::UnknownParameter)?;
        Ok(ParameterValue::Float(val))
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let (mut params, raw_state) = self.validate_and_decode(state)?;
        let desc = self
            .parameters
            .iter()
            .find(|d| d.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        desc.validate(value)?;
        let ParameterValue::Float(float_val) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };

        // Apply parameter edit on a control-side instance and re-capture opaque state.
        let mut inst = self
            .module
            .create_instance(48_000)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        if (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0 && !raw_state.is_empty() {
            inst.set_state(raw_state)
                .map_err(|_| DeviceContractError::MalformedState)?;
        }
        let ev = [AbiParamEvent {
            sample_offset: 0,
            parameter_id: parameter.value(),
            value: float_val,
            reserved: 0,
        }];
        let input = [[0.0f32; 2]; 1];
        let mut output = [[0.0f32; 2]; 1];
        inst.process(&input, &mut output, &ev, &[])
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;

        let new_raw = if (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0 {
            let mut buf = vec![0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize];
            let len = inst
                .get_state(&mut buf)
                .map_err(|_| DeviceContractError::MalformedState)?;
            buf.truncate(len);
            buf
        } else {
            raw_state.to_vec()
        };

        for (id, val) in &mut params {
            if *id == parameter.value() {
                *val = float_val;
            }
        }
        let payload = encode_param_wrapped_state(&self.identity, &params, &new_raw)?;
        NativeDeviceState::new(self.type_id, self.descriptor.state_version, payload)?
            .with_host_metadata_from(state)
    }
}

/// Generic `NativeDeviceDefinition` backed by an installed `.hexdevice` audio
/// effect package (e.g. `hex.pkg.utility`, `hex.pkg.soft_clip`).
pub struct InstalledPackageEffect {
    shared: SharedPackageDefinition,
}

impl InstalledPackageEffect {
    /// Load an admitted effect package `.dylib` on control. Never call on Play.
    pub fn load_on_control(
        dylib_path: &Path,
        publisher_id: impl Into<String>,
        package_sha256: [u8; 32],
    ) -> Result<Self, DeviceContractError> {
        let shared = SharedPackageDefinition::load(
            dylib_path,
            publisher_id.into(),
            package_sha256,
            DEVICE_KIND_AUDIO_EFFECT,
        )?;
        Ok(Self { shared })
    }

    pub fn package_identity(&self) -> &PackageIdentityEnvelope {
        &self.shared.identity
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.shared.parameter_names
    }

    pub fn active_module_leases(&self) -> usize {
        self.shared.module.active_lease_count()
    }
}

impl NativeDeviceMetadata for InstalledPackageEffect {
    fn type_id(&self) -> DeviceTypeId {
        self.shared.type_id
    }

    fn state_version(&self) -> u32 {
        self.shared.descriptor.state_version
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        let _ = self.shared.validate_and_decode(state)?;
        Ok(self.shared.parameters.clone())
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.shared.default_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = self.shared.validate_and_decode(state)?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        self.shared.read_parameter(state, parameter)
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.shared.patch_parameter(state, parameter, value)
    }
}

impl NativeDeviceDefinition for InstalledPackageEffect {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = self.shared.validate_and_decode(state)?;
        Ok(StereoMatrix::identity())
    }

    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        let (params, raw_state) = self.shared.validate_and_decode(state)?;
        if !state.asset_references().is_empty()
            && (self.shared.descriptor.capabilities & CAP_BORROWED_RESOURCES) == 0
        {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        context.validate_for(state)?;
        let mut base_values = [0.0f32; PACKAGE_ABI_MAX_PARAMS as usize];
        for (idx, desc) in self.shared.parameters.iter().enumerate() {
            let val = params
                .iter()
                .find(|&&(id, _)| id == desc.id.value())
                .map(|&(_, v)| v)
                .unwrap_or(match desc.default {
                    ParameterValue::Float(v) => v,
                    ParameterValue::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    ParameterValue::Choice(c) => c as f32,
                });
            base_values[idx] = val;
        }
        let factory = PreparedInstalledEffect {
            module: self.shared.module.clone(),
            descriptor: self.shared.descriptor,
            sample_rate: context.sample_rate,
            initial_raw_state: raw_state.to_vec(),
            parameters: self.shared.parameters.clone(),
            base_values,
        };
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(factory))))
    }
}

struct PreparedInstalledEffect {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: Vec<u8>,
    parameters: Vec<ParameterDescriptor>,
    base_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
}

impl PreparedOwnedProcessorRuntime for PreparedInstalledEffect {
    fn supports_smoothed_bypass(&self) -> bool {
        self.descriptor.reported_latency_frames == 0
    }

    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        if (self.descriptor.capabilities & CAP_PARAMETER_EVENTS) == 0 {
            return None;
        }
        self.parameters
            .iter()
            .copied()
            .find(|desc| desc.id == parameter)
    }

    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.parameter_automation_descriptor(parameter).is_some()
    }

    fn runtime_version(&self) -> u32 {
        self.descriptor.abi_version
    }

    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<InstalledEffectRuntime>()
    }

    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let mut committed = self
            .module
            .create_instance(self.sample_rate)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        let mut speculative = self
            .module
            .create_instance(self.sample_rate)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        if (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0
            && !self.initial_raw_state.is_empty()
        {
            committed
                .set_state(&self.initial_raw_state)
                .map_err(|_| DeviceContractError::MalformedState)?;
            speculative
                .set_state(&self.initial_raw_state)
                .map_err(|_| DeviceContractError::MalformedState)?;
        }
        let mut baseline_state = [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize];
        let baseline_len = self.initial_raw_state.len().min(baseline_state.len());
        baseline_state[..baseline_len].copy_from_slice(&self.initial_raw_state[..baseline_len]);
        let mut param_descriptors = [ParameterDescriptor {
            id: ParameterId::from_static(1),
            label: "param",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        }; PACKAGE_ABI_MAX_PARAMS as usize];
        let param_count = self.parameters.len().min(param_descriptors.len());
        param_descriptors[..param_count].copy_from_slice(&self.parameters[..param_count]);
        Ok(Box::new(InstalledEffectRuntime {
            committed,
            speculative,
            has_state_persistence: (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0,
            supports_param_events: (self.descriptor.capabilities & CAP_PARAMETER_EVENTS) != 0,
            supports_borrowed_resources: (self.descriptor.capabilities & CAP_BORROWED_RESOURCES)
                != 0,
            param_count,
            param_descriptors,
            base_values: self.base_values,
            committed_values: self.base_values,
            speculative_values: self.base_values,
            baseline_state,
            baseline_len,
            state_scratch: [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
            param_scratch: [AbiParamEvent {
                sample_offset: 0,
                parameter_id: 0,
                value: 0.0,
                reserved: 0,
            }; PACKAGE_ABI_MAX_PARAM_EVENTS as usize],
            audio_scratch: [[0.0; 2]; PACKAGE_ABI_MAX_FRAMES as usize],
            faulted: false,
        }))
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.descriptor.tail_frames),
        }
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.descriptor.reported_latency_frames)
    }
}

struct InstalledEffectRuntime {
    committed: PackageInstance,
    speculative: PackageInstance,
    has_state_persistence: bool,
    supports_param_events: bool,
    supports_borrowed_resources: bool,
    param_count: usize,
    param_descriptors: [ParameterDescriptor; PACKAGE_ABI_MAX_PARAMS as usize],
    base_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    committed_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    speculative_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    baseline_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    baseline_len: usize,
    state_scratch: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    param_scratch: [AbiParamEvent; PACKAGE_ABI_MAX_PARAM_EVENTS as usize],
    audio_scratch: [StereoFrame; PACKAGE_ABI_MAX_FRAMES as usize],
    faulted: bool,
}

impl InstalledEffectRuntime {
    fn sync_instance(
        has_state: bool,
        src: &PackageInstance,
        dst: &mut PackageInstance,
        scratch: &mut [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    ) {
        if has_state && let Ok(len) = src.get_state(scratch) {
            let _ = dst.set_state(&scratch[..len]);
        }
    }

    fn param_index(&self, id: ParameterId) -> Option<usize> {
        self.param_descriptors[..self.param_count]
            .iter()
            .position(|desc| desc.id == id)
    }

    fn param_value_to_f32(
        desc: ParameterDescriptor,
        value: ParameterValue,
    ) -> Result<f32, DeviceContractError> {
        desc.validate(value)?;
        Ok(match value {
            ParameterValue::Float(v) => v,
            ParameterValue::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            ParameterValue::Choice(c) => c as f32,
        })
    }
}

impl OwnedProcessorRuntime for InstalledEffectRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.supports_param_events && self.param_index(parameter).is_some()
    }

    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        if !self.supports_param_events {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        // Validate all declared lane IDs before mutating any state or audio.
        let lane_ids = automation.parameter_ids();
        for &id in lane_ids {
            if self.param_index(id).is_none() {
                return Err(DeviceContractError::UnknownParameter);
            }
        }
        if frames.is_empty() {
            return Ok(());
        }
        // Pre-validate every automated sample value so errors never partially mutate PCM.
        for sample_offset in 0..frames.len() {
            for &id in lane_ids {
                if let Some(val) = automation.value_at(id, sample_offset)
                    && let Some(slot) = self.param_index(id)
                {
                    let _ = Self::param_value_to_f32(self.param_descriptors[slot], val)?;
                }
            }
        }
        if self.faulted {
            frames.fill([0.0; 2]);
            return Ok(());
        }

        let max_chunk = PACKAGE_ABI_MAX_FRAMES as usize;
        let mut seg_start = 0usize;
        let mut pending_count = 0usize;
        for sample_offset in 0..frames.len() {
            // Count how many parameter changes occur at this sample offset.
            let mut changes_at_sample = 0usize;
            for slot in 0..self.param_count {
                let desc = self.param_descriptors[slot];
                let target = match automation.value_at(desc.id, sample_offset) {
                    Some(val) => Self::param_value_to_f32(desc, val)?,
                    None => self.base_values[slot],
                };
                if target.to_bits() != self.speculative_values[slot].to_bits() {
                    changes_at_sample += 1;
                }
            }
            // Split sub-block cleanly whenever adding events would exceed the fixed
            // C ABI event queue or frame bound — zero drops, zero allocations.
            let chunk_len = sample_offset - seg_start;
            if chunk_len > 0
                && (chunk_len >= max_chunk
                    || pending_count + changes_at_sample > self.param_scratch.len())
            {
                let chunk = &mut frames[seg_start..sample_offset];
                if call_process_with_borrowed_resources(
                    &mut self.speculative,
                    self.supports_borrowed_resources,
                    chunk,
                    &mut self.audio_scratch[..chunk.len()],
                    &self.param_scratch[..pending_count],
                    &[],
                    resources,
                )
                .is_err()
                {
                    self.faulted = true;
                    frames[seg_start..].fill([0.0; 2]);
                    return Ok(());
                }
                chunk.copy_from_slice(&self.audio_scratch[..chunk.len()]);
                seg_start = sample_offset;
                pending_count = 0;
            }
            let local_offset = (sample_offset - seg_start) as u32;
            for slot in 0..self.param_count {
                let desc = self.param_descriptors[slot];
                let target = match automation.value_at(desc.id, sample_offset) {
                    Some(val) => Self::param_value_to_f32(desc, val)?,
                    None => self.base_values[slot],
                };
                if target.to_bits() != self.speculative_values[slot].to_bits() {
                    self.speculative_values[slot] = target;
                    if pending_count < self.param_scratch.len() {
                        self.param_scratch[pending_count] = AbiParamEvent {
                            sample_offset: local_offset,
                            parameter_id: desc.id.value(),
                            value: target,
                            reserved: 0,
                        };
                        pending_count += 1;
                    }
                }
            }
        }
        // Flush trailing segment.
        if seg_start < frames.len() {
            let chunk = &mut frames[seg_start..];
            if call_process_with_borrowed_resources(
                &mut self.speculative,
                self.supports_borrowed_resources,
                chunk,
                &mut self.audio_scratch[..chunk.len()],
                &self.param_scratch[..pending_count],
                &[],
                resources,
            )
            .is_err()
            {
                self.faulted = true;
                chunk.fill([0.0; 2]);
                return Ok(());
            }
            chunk.copy_from_slice(&self.audio_scratch[..chunk.len()]);
        }
        Ok(())
    }

    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn begin_block(&mut self) {
        self.speculative_values = self.committed_values;
        Self::sync_instance(
            self.has_state_persistence,
            &self.committed,
            &mut self.speculative,
            &mut self.state_scratch,
        );
    }

    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        if self.faulted {
            frames.fill([0.0; 2]);
            return;
        }
        // Restore authored base parameters if a prior automated block left a different effective value.
        let mut pending_count = 0usize;
        for slot in 0..self.param_count {
            if self.speculative_values[slot].to_bits() != self.base_values[slot].to_bits() {
                self.speculative_values[slot] = self.base_values[slot];
                if pending_count < self.param_scratch.len() {
                    self.param_scratch[pending_count] = AbiParamEvent {
                        sample_offset: 0,
                        parameter_id: self.param_descriptors[slot].id.value(),
                        value: self.base_values[slot],
                        reserved: 0,
                    };
                    pending_count += 1;
                }
            }
        }
        let max_chunk = PACKAGE_ABI_MAX_FRAMES as usize;
        for (chunk_idx, chunk) in frames.chunks_mut(max_chunk).enumerate() {
            let len = chunk.len();
            let events = if chunk_idx == 0 {
                &self.param_scratch[..pending_count]
            } else {
                &[]
            };
            if call_process_with_borrowed_resources(
                &mut self.speculative,
                self.supports_borrowed_resources,
                chunk,
                &mut self.audio_scratch[..len],
                events,
                &[],
                resources,
            )
            .is_err()
            {
                self.faulted = true;
                chunk.fill([0.0; 2]);
                return;
            }
            chunk.copy_from_slice(&self.audio_scratch[..len]);
        }
    }

    fn commit_block(&mut self) {
        self.committed_values = self.speculative_values;
        Self::sync_instance(
            self.has_state_persistence,
            &self.speculative,
            &mut self.committed,
            &mut self.state_scratch,
        );
    }

    fn rollback_block(&mut self) {
        self.speculative_values = self.committed_values;
        Self::sync_instance(
            self.has_state_persistence,
            &self.committed,
            &mut self.speculative,
            &mut self.state_scratch,
        );
    }

    fn reset(&mut self) {
        self.faulted = false;
        self.committed_values = self.base_values;
        self.speculative_values = self.base_values;
        if self.has_state_persistence && self.baseline_len > 0 {
            let _ = self
                .committed
                .set_state(&self.baseline_state[..self.baseline_len]);
            let _ = self
                .speculative
                .set_state(&self.baseline_state[..self.baseline_len]);
        }
    }

    fn committed_is_quiescent(&self) -> bool {
        true
    }

    fn speculative_is_quiescent(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Generic `NativeInstrumentDefinition` backed by an installed `.hexdevice`
/// instrument package (e.g. `hex.pkg.sine`).
pub struct InstalledPackageInstrument {
    shared: SharedPackageDefinition,
}

impl InstalledPackageInstrument {
    /// Load an admitted instrument package `.dylib` on control. Never call on Play.
    pub fn load_on_control(
        dylib_path: &Path,
        publisher_id: impl Into<String>,
        package_sha256: [u8; 32],
    ) -> Result<Self, DeviceContractError> {
        let shared = SharedPackageDefinition::load(
            dylib_path,
            publisher_id.into(),
            package_sha256,
            DEVICE_KIND_INSTRUMENT,
        )?;
        Ok(Self { shared })
    }

    pub fn package_identity(&self) -> &PackageIdentityEnvelope {
        &self.shared.identity
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.shared.parameter_names
    }

    pub fn active_module_leases(&self) -> usize {
        self.shared.module.active_lease_count()
    }
}

impl NativeDeviceMetadata for InstalledPackageInstrument {
    fn type_id(&self) -> DeviceTypeId {
        self.shared.type_id
    }

    fn state_version(&self) -> u32 {
        self.shared.descriptor.state_version
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::STEREO_INSTRUMENT_PORTS
    }

    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        let _ = self.shared.validate_and_decode(state)?;
        Ok(self.shared.parameters.clone())
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.shared.default_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let _ = self.shared.validate_and_decode(state)?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        self.shared.read_parameter(state, parameter)
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.shared.patch_parameter(state, parameter, value)
    }
}

impl NativeInstrumentDefinition for InstalledPackageInstrument {
    fn supports_asset_resources(&self) -> bool {
        (self.shared.descriptor.capabilities & CAP_BORROWED_RESOURCES) != 0
    }

    fn resource_factory_inline_bound(&self) -> Option<usize> {
        self.supports_asset_resources()
            .then_some(std::mem::size_of::<PreparedInstalledInstrumentFactory>())
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let (params, raw_state) = self.shared.validate_and_decode(state)?;
        if !state.asset_references().is_empty() && !self.supports_asset_resources() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        context.validate_for(state)?;
        let mut sorted_params = self.shared.parameters.clone();
        sorted_params.sort_by_key(|desc| desc.id);
        let mut base_values = [0.0f32; PACKAGE_ABI_MAX_PARAMS as usize];
        for (idx, desc) in sorted_params.iter().enumerate() {
            let val = params
                .iter()
                .find(|&&(id, _)| id == desc.id.value())
                .map(|&(_, v)| v)
                .unwrap_or(match desc.default {
                    ParameterValue::Float(v) => v,
                    ParameterValue::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    ParameterValue::Choice(c) => c as f32,
                });
            base_values[idx] = val;
        }
        // Precompute exact per-MIDI-note detune cents from the host TuningTable on control
        // so the audio callback performs zero transcendental tuning math.
        let mut note_detune_cents = [0.0f32; 128];
        let freqs = context.tuning.frequencies();
        for (pitch, slot) in note_detune_cents.iter_mut().enumerate() {
            let standard_hz = 440.0 * 2.0_f64.powf((pitch as f64 - 69.0) / 12.0);
            let tuned_hz = freqs[pitch];
            let cents = if tuned_hz.is_finite() && tuned_hz > 0.0 {
                (1200.0 * (tuned_hz / standard_hz).log2()) as f32
            } else {
                0.0
            };
            *slot = if cents.abs() < 1e-4 { 0.0 } else { cents };
        }
        let mut initial_raw_state = [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize];
        let initial_raw_state_len = raw_state.len().min(initial_raw_state.len());
        initial_raw_state[..initial_raw_state_len]
            .copy_from_slice(&raw_state[..initial_raw_state_len]);
        let mut param_descriptors = [ParameterDescriptor {
            id: ParameterId::from_static(1),
            label: "param",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        }; PACKAGE_ABI_MAX_PARAMS as usize];
        let param_count = sorted_params.len().min(param_descriptors.len());
        param_descriptors[..param_count].copy_from_slice(&sorted_params[..param_count]);
        Ok(Box::new(PreparedInstalledInstrument {
            module: self.shared.module.clone(),
            descriptor: self.shared.descriptor,
            sample_rate: context.sample_rate,
            initial_raw_state,
            initial_raw_state_len,
            identity_digest: self.shared.identity.package_sha256,
            parameters: param_descriptors,
            param_count,
            base_values,
            note_detune_cents,
            required_resource_count: state.asset_references().len(),
        }))
    }
}

struct PreparedInstalledInstrument {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    initial_raw_state_len: usize,
    identity_digest: [u8; 32],
    parameters: [ParameterDescriptor; PACKAGE_ABI_MAX_PARAMS as usize],
    param_count: usize,
    base_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    note_detune_cents: [f32; 128],
    required_resource_count: usize,
}

impl PreparedNativeInstrument for PreparedInstalledInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.descriptor.reported_latency_frames)
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
        Ok(Some(Box::new(PreparedInstalledInstrumentFactory {
            module: self.module.clone(),
            descriptor: self.descriptor,
            sample_rate: self.sample_rate,
            initial_raw_state: self.initial_raw_state,
            initial_raw_state_len: self.initial_raw_state_len,
            identity_digest: self.identity_digest,
            parameters: self.parameters,
            param_count: self.param_count,
            base_values: self.base_values,
            note_detune_cents: self.note_detune_cents,
            required_resource_count: self.required_resource_count,
        })))
    }
}

/// Inline-only immutable factory for installed instrument packages (`#3002`).
/// Owns zero variable heap buffers; holds only its `PackageModule` generation
/// lease and fixed-size state/parameter metadata.
struct PreparedInstalledInstrumentFactory {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    initial_raw_state_len: usize,
    identity_digest: [u8; 32],
    parameters: [ParameterDescriptor; PACKAGE_ABI_MAX_PARAMS as usize],
    param_count: usize,
    base_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    note_detune_cents: [f32; 128],
    required_resource_count: usize,
}

impl PreparedMutableInstrumentRuntime for PreparedInstalledInstrumentFactory {
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        ((self.descriptor.capabilities & CAP_BORROWED_RESOURCES) != 0)
            .then_some(std::mem::size_of::<Self>())
    }

    fn parameter_automation_descriptors(&self) -> &[ParameterDescriptor] {
        if (self.descriptor.capabilities & CAP_PARAMETER_EVENTS) != 0 {
            &self.parameters[..self.param_count]
        } else {
            &[]
        }
    }

    fn supports_note_pressure(&self) -> bool {
        (self.descriptor.capabilities & CAP_NOTE_EXPRESSION) != 0
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.descriptor.reported_latency_frames)
    }

    fn runtime_version(&self) -> u32 {
        self.descriptor.abi_version
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<InstalledInstrumentRuntime>()
    }

    fn max_events_per_block(&self) -> usize {
        PACKAGE_ABI_MAX_NOTE_EVENTS as usize
    }

    fn max_active_occurrences(&self) -> usize {
        MAX_PACKAGE_VOICES
    }

    fn max_release_tail_frames(&self) -> u64 {
        u64::from(self.descriptor.tail_frames)
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let mut instance = self.module.create_instance(self.sample_rate).ok();
        if let Some(inst) = instance.as_mut()
            && (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0
            && self.initial_raw_state_len > 0
        {
            let _ = inst.set_state(&self.initial_raw_state[..self.initial_raw_state_len]);
        }
        Box::new(InstalledInstrumentRuntime {
            instance,
            sample_rate: self.sample_rate,
            capabilities: self.descriptor.capabilities,
            identity_digest: self.identity_digest,
            has_state_persistence: (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0,
            supports_borrowed_resources: (self.descriptor.capabilities & CAP_BORROWED_RESOURCES)
                != 0,
            required_resource_count: self.required_resource_count,
            max_release_tail_frames: u64::from(self.descriptor.tail_frames),
            tail_frames_remaining: 0,
            param_count: self.param_count,
            param_descriptors: self.parameters,
            base_values: self.base_values,
            current_values: self.base_values,
            note_detune_cents: self.note_detune_cents,
            baseline_state: self.initial_raw_state,
            baseline_len: self.initial_raw_state_len,
            active_occurrences: [None; MAX_PACKAGE_VOICES],
            param_scratch: [AbiParamEvent {
                sample_offset: 0,
                parameter_id: 0,
                value: 0.0,
                reserved: 0,
            }; PACKAGE_ABI_MAX_PARAM_EVENTS as usize],
            note_scratch: [AbiNoteEvent {
                sample_offset: 0,
                kind: 0,
                note: 0,
                channel: 0,
                reserved0: 0,
                velocity: 0.0,
                detune_cents: 0.0,
                reserved1: 0,
            }; PACKAGE_ABI_MAX_NOTE_EVENTS as usize],
            silence_in: [[0.0; 2]; PACKAGE_ABI_MAX_FRAMES as usize],
            audio_scratch: [[0.0; 2]; PACKAGE_ABI_MAX_FRAMES as usize],
            state_scratch: [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
            faulted: false,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct InstalledInstrumentRuntime {
    instance: Option<PackageInstance>,
    sample_rate: u32,
    capabilities: u64,
    identity_digest: [u8; 32],
    has_state_persistence: bool,
    supports_borrowed_resources: bool,
    required_resource_count: usize,
    max_release_tail_frames: u64,
    tail_frames_remaining: u64,
    param_count: usize,
    param_descriptors: [ParameterDescriptor; PACKAGE_ABI_MAX_PARAMS as usize],
    base_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    current_values: [f32; PACKAGE_ABI_MAX_PARAMS as usize],
    note_detune_cents: [f32; 128],
    baseline_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    baseline_len: usize,
    active_occurrences: [Option<(u64, u8)>; MAX_PACKAGE_VOICES],
    param_scratch: [AbiParamEvent; PACKAGE_ABI_MAX_PARAM_EVENTS as usize],
    note_scratch: [AbiNoteEvent; PACKAGE_ABI_MAX_NOTE_EVENTS as usize],
    silence_in: [StereoFrame; PACKAGE_ABI_MAX_FRAMES as usize],
    audio_scratch: [StereoFrame; PACKAGE_ABI_MAX_FRAMES as usize],
    state_scratch: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    faulted: bool,
}

impl InstalledInstrumentRuntime {
    fn supports_param_events(&self) -> bool {
        (self.capabilities & CAP_PARAMETER_EVENTS) != 0 && self.param_count > 0
    }

    fn supports_note_expression(&self) -> bool {
        (self.capabilities & CAP_NOTE_EXPRESSION) != 0
    }

    fn supports_channel_expression(&self) -> bool {
        (self.capabilities & CAP_CHANNEL_EXPRESSION) != 0
    }

    fn param_index(&self, id: ParameterId) -> Option<usize> {
        self.param_descriptors[..self.param_count]
            .iter()
            .position(|desc| desc.id == id)
    }

    fn translate_event(
        &mut self,
        ev: NativeInstrumentEvent,
        local_offset: u32,
    ) -> Option<AbiNoteEvent> {
        match ev {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if let Some(slot) = self.active_occurrences.iter_mut().find(|s| s.is_none()) {
                    *slot = Some((occurrence_id, pitch));
                }
                let detune_cents = self.note_detune_cents[usize::from(pitch.min(127))];
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_ON,
                    note: pitch,
                    channel: 0,
                    reserved0: 0,
                    velocity: f32::from(velocity) / 127.0,
                    detune_cents,
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                let slot = self
                    .active_occurrences
                    .iter_mut()
                    .find(|s| matches!(s, Some((id, _)) if *id == occurrence_id))?;
                let pitch = slot.map(|(_, p)| p).unwrap_or(0);
                *slot = None;
                if self.max_release_tail_frames > 0 {
                    self.tail_frames_remaining = self.max_release_tail_frames;
                }
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_OFF,
                    note: pitch,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.0,
                    detune_cents: 0.0,
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::NotePressure {
                occurrence_id,
                value,
                ..
            } => {
                let pitch = self.active_occurrences.iter().find_map(|s| {
                    s.filter(|(id, _)| *id == occurrence_id.get())
                        .map(|(_, p)| p)
                })?;
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_PRESSURE,
                    note: pitch,
                    channel: 0,
                    reserved0: 0,
                    velocity: f32::from(value) / 127.0,
                    detune_cents: 0.0,
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                let pitch = self.active_occurrences.iter().find_map(|s| {
                    s.filter(|(id, _)| *id == occurrence_id.get())
                        .map(|(_, p)| p)
                })?;
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_PITCH_BEND,
                    note: pitch,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.0,
                    detune_cents: f32::from(cents),
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::NoteTimbre {
                occurrence_id,
                value,
                ..
            } => {
                let pitch = self.active_occurrences.iter().find_map(|s| {
                    s.filter(|(id, _)| *id == occurrence_id.get())
                        .map(|(_, p)| p)
                })?;
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_CC,
                    note: 74,
                    channel: pitch,
                    reserved0: 0,
                    velocity: f32::from(value) / 127.0,
                    detune_cents: 0.0,
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                let norm = NativeInstrumentExpression::PitchBend(value).normalized();
                Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_PITCH_BEND,
                    note: 0xFF,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.0,
                    detune_cents: norm * 200.0,
                    reserved1: 0,
                })
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => Some(AbiNoteEvent {
                sample_offset: local_offset,
                kind: NOTE_EVENT_CC,
                note: cc,
                channel: 0,
                reserved0: 0,
                velocity: f32::from(value) / 127.0,
                detune_cents: 0.0,
                reserved1: 0,
            }),
            NativeInstrumentEvent::Expression { expression, .. } => match expression {
                NativeInstrumentExpression::PitchBend(_) => Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_PITCH_BEND,
                    note: 0xFF,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.0,
                    detune_cents: expression.normalized() * 200.0,
                    reserved1: 0,
                }),
                NativeInstrumentExpression::ChannelPressure(_) => Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_PRESSURE,
                    note: 0xFF,
                    channel: 0,
                    reserved0: 0,
                    velocity: expression.normalized(),
                    detune_cents: 0.0,
                    reserved1: 0,
                }),
                NativeInstrumentExpression::ModWheel(_) => Some(AbiNoteEvent {
                    sample_offset: local_offset,
                    kind: NOTE_EVENT_CC,
                    note: 1,
                    channel: 0,
                    reserved0: 0,
                    velocity: expression.normalized(),
                    detune_cents: 0.0,
                    reserved1: 0,
                }),
            },
        }
    }

    fn process_with_optional_automation(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        values: Option<&dyn NativeParameterAutomation>,
        resources: &dyn DeviceResources,
    ) {
        if self.faulted || frames.is_empty() || self.instance.is_none() {
            return;
        }
        let max_chunk = PACKAGE_ABI_MAX_FRAMES as usize;
        let mut seg_start = 0usize;
        let mut ev_cursor = 0usize;
        let mut param_count = 0usize;
        let mut note_count = 0usize;

        for sample_offset in 0..frames.len() {
            // Count parameter changes and MIDI events arriving at `sample_offset`.
            let mut param_changes = 0usize;
            if self.supports_param_events() {
                for slot in 0..self.param_count {
                    let desc = self.param_descriptors[slot];
                    let target = values
                        .and_then(|auto| auto.value_at(desc.id, sample_offset))
                        .and_then(|v| InstalledEffectRuntime::param_value_to_f32(desc, v).ok())
                        .unwrap_or(self.base_values[slot]);
                    if target.to_bits() != self.current_values[slot].to_bits() {
                        param_changes += 1;
                    }
                }
            }
            let mut midi_at_sample = 0usize;
            while ev_cursor + midi_at_sample < events.len()
                && events[ev_cursor + midi_at_sample].frame_offset() == sample_offset
            {
                midi_at_sample += 1;
            }

            let chunk_len = sample_offset - seg_start;
            if chunk_len > 0
                && (chunk_len >= max_chunk
                    || param_count + param_changes > self.param_scratch.len()
                    || note_count + midi_at_sample > self.note_scratch.len())
            {
                let Some(inst) = self.instance.as_mut() else {
                    return;
                };
                if call_process_with_borrowed_resources(
                    inst,
                    self.supports_borrowed_resources,
                    &self.silence_in[..chunk_len],
                    &mut self.audio_scratch[..chunk_len],
                    &self.param_scratch[..param_count],
                    &self.note_scratch[..note_count],
                    resources,
                )
                .is_err()
                {
                    self.faulted = true;
                    return;
                }
                for i in 0..chunk_len {
                    frames[seg_start + i][0] += self.audio_scratch[i][0];
                    frames[seg_start + i][1] += self.audio_scratch[i][1];
                }
                if self.active_occurrences.iter().all(Option::is_none) {
                    self.tail_frames_remaining =
                        self.tail_frames_remaining.saturating_sub(chunk_len as u64);
                }
                seg_start = sample_offset;
                param_count = 0;
                note_count = 0;
            }

            let local_offset = (sample_offset - seg_start) as u32;
            // Parameter values apply BEFORE MIDI events at the same sample offset.
            if self.supports_param_events() {
                for slot in 0..self.param_count {
                    let desc = self.param_descriptors[slot];
                    let target = values
                        .and_then(|auto| auto.value_at(desc.id, sample_offset))
                        .and_then(|v| InstalledEffectRuntime::param_value_to_f32(desc, v).ok())
                        .unwrap_or(self.base_values[slot]);
                    if target.to_bits() != self.current_values[slot].to_bits() {
                        self.current_values[slot] = target;
                        if param_count < self.param_scratch.len() {
                            self.param_scratch[param_count] = AbiParamEvent {
                                sample_offset: local_offset,
                                parameter_id: desc.id.value(),
                                value: target,
                                reserved: 0,
                            };
                            param_count += 1;
                        }
                    }
                }
            }
            for _ in 0..midi_at_sample {
                let ev = events[ev_cursor];
                ev_cursor += 1;
                if let Some(abi_ev) = self.translate_event(ev, local_offset)
                    && note_count < self.note_scratch.len()
                {
                    self.note_scratch[note_count] = abi_ev;
                    note_count += 1;
                }
            }
        }

        if seg_start < frames.len() {
            let chunk_len = frames.len() - seg_start;
            let Some(inst) = self.instance.as_mut() else {
                return;
            };
            if call_process_with_borrowed_resources(
                inst,
                self.supports_borrowed_resources,
                &self.silence_in[..chunk_len],
                &mut self.audio_scratch[..chunk_len],
                &self.param_scratch[..param_count],
                &self.note_scratch[..note_count],
                resources,
            )
            .is_err()
            {
                self.faulted = true;
                return;
            }
            for i in 0..chunk_len {
                frames[seg_start + i][0] += self.audio_scratch[i][0];
                frames[seg_start + i][1] += self.audio_scratch[i][1];
            }
            if self.active_occurrences.iter().all(Option::is_none) {
                self.tail_frames_remaining =
                    self.tail_frames_remaining.saturating_sub(chunk_len as u64);
            }
        }
    }
}

impl InstrumentParameterRuntime for InstalledInstrumentRuntime {
    fn descriptors(&self) -> &[ParameterDescriptor] {
        &self.param_descriptors[..self.param_count]
    }

    fn copy_targets(
        &self,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.supports_param_events() {
            return Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation);
        }
        if output.len() != self.param_count {
            return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
        }
        for (idx, slot) in output.iter_mut().enumerate() {
            *slot = Some(ParameterValue::Float(self.current_values[idx]));
        }
        Ok(())
    }

    fn preflight(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.supports_param_events() {
            return Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation);
        }
        if !gains.is_empty() {
            return Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain);
        }
        self.preflight_events(events, frame_count, resources)?;
        for &id in values.parameter_ids() {
            if self.param_index(id).is_none() {
                return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
            }
        }
        for sample_offset in 0..frame_count {
            for &id in values.parameter_ids() {
                if let Some(val) = values.value_at(id, sample_offset)
                    && let Some(slot) = self.param_index(id)
                    && self.param_descriptors[slot].validate(val).is_err()
                {
                    return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
                }
            }
        }
        Ok(())
    }

    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _gains: &[NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) {
        self.process_with_optional_automation(events, frames, Some(values), resources);
    }
}

impl MutableInstrumentRuntime for InstalledInstrumentRuntime {
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        self.supports_param_events()
            .then_some(self as &mut dyn InstrumentParameterRuntime)
    }

    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        self.supports_param_events()
            .then_some(self as &dyn InstrumentParameterRuntime)
    }

    fn supports_note_pressure(&self) -> bool {
        self.supports_note_expression()
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if self.required_resource_count > 0 && resources.len() < self.required_resource_count {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if events.len() > PACKAGE_ABI_MAX_NOTE_EVENTS as usize {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut active = self.active_occurrences;
        let mut last_offset = 0usize;
        for &ev in events {
            let offset = ev.frame_offset();
            if offset >= frame_count || offset < last_offset {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            last_offset = offset;
            match ev {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    velocity,
                    ..
                } => {
                    if occurrence_id == 0 || pitch > 127 || velocity == 0 || velocity > 127 {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    if active
                        .iter()
                        .any(|s| matches!(s, Some((id, _)) if *id == occurrence_id))
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    let Some(slot) = active.iter_mut().find(|s| s.is_none()) else {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    };
                    *slot = Some((occurrence_id, pitch));
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if occurrence_id == 0 {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    let Some(slot) = active
                        .iter_mut()
                        .find(|s| matches!(s, Some((id, _)) if *id == occurrence_id))
                    else {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    };
                    *slot = None;
                }
                NativeInstrumentEvent::NotePressure {
                    occurrence_id,
                    value,
                    ..
                } => {
                    if !self.supports_note_expression() {
                        return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
                    }
                    if value > 127
                        || !active
                            .iter()
                            .any(|s| matches!(s, Some((id, _)) if *id == occurrence_id.get()))
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
                NativeInstrumentEvent::NotePitchBend {
                    occurrence_id,
                    cents,
                    ..
                } => {
                    if !self.supports_note_expression() {
                        return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
                    }
                    if !(-4800..=4800).contains(&cents)
                        || !active
                            .iter()
                            .any(|s| matches!(s, Some((id, _)) if *id == occurrence_id.get()))
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
                NativeInstrumentEvent::NoteTimbre {
                    occurrence_id,
                    value,
                    ..
                } => {
                    if !self.supports_note_expression() {
                        return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
                    }
                    if value > 127
                        || !active
                            .iter()
                            .any(|s| matches!(s, Some((id, _)) if *id == occurrence_id.get()))
                    {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
                NativeInstrumentEvent::Expression { expression, .. } => {
                    if !self.supports_channel_expression() {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    if !expression.is_valid() {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
                NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                    if !self.supports_channel_expression() {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    if cc > 127 || value > 127 {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
                NativeInstrumentEvent::PitchBend { value, .. } => {
                    if !self.supports_channel_expression() {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                    if !(-8192..=8191).contains(&value) {
                        return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                    }
                }
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
        self.process_with_optional_automation(events, frames, None, resources);
    }

    fn reset(&mut self) {
        self.faulted = false;
        self.tail_frames_remaining = 0;
        self.current_values = self.base_values;
        // Emit note-off / all-notes-off for every active note so C module clears voices cleanly.
        let mut count = 0usize;
        for slot in &mut self.active_occurrences {
            if let Some((_, pitch)) = slot.take()
                && count < self.note_scratch.len()
            {
                self.note_scratch[count] = AbiNoteEvent {
                    sample_offset: 0,
                    kind: NOTE_EVENT_OFF,
                    note: pitch,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.0,
                    detune_cents: 0.0,
                    reserved1: 0,
                };
                count += 1;
            }
        }
        if self.supports_channel_expression() && count < self.note_scratch.len() {
            self.note_scratch[count] = AbiNoteEvent {
                sample_offset: 0,
                kind: NOTE_EVENT_ALL_NOTES_OFF,
                note: 0xFF,
                channel: 0,
                reserved0: 0,
                velocity: 0.0,
                detune_cents: 0.0,
                reserved1: 0,
            };
            count += 1;
        }
        if let Some(inst) = self.instance.as_mut() {
            if count > 0 {
                let _ = inst.process(
                    &self.silence_in[..1],
                    &mut self.audio_scratch[..1],
                    &[],
                    &self.note_scratch[..count],
                );
            }
            if self.has_state_persistence && self.baseline_len > 0 {
                let _ = inst.set_state(&self.baseline_state[..self.baseline_len]);
            }
        }
    }

    fn is_quiescent(&self) -> bool {
        self.active_occurrences.iter().all(Option::is_none) && self.tail_frames_remaining == 0
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|src| {
            src.sample_rate == self.sample_rate && src.identity_digest == self.identity_digest
        })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(src) = previous.as_any().downcast_ref::<Self>() {
            self.active_occurrences = src.active_occurrences;
            self.tail_frames_remaining = src.tail_frames_remaining;
            self.current_values = src.current_values;
            self.faulted = src.faulted;
            if self.has_state_persistence
                && let (Some(src_inst), Some(dst_inst)) = (&src.instance, &mut self.instance)
                && let Ok(len) = src_inst.get_state(&mut self.state_scratch)
            {
                let _ = dst_inst.set_state(&self.state_scratch[..len]);
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

/// Unavailable placeholder effect adapter (`#2993`). Preserves opaque package
/// state, pinned `PackageIdentityEnvelope`, and asset references verbatim
/// without loading or executing module code. Refuses active audio processing
/// (`ForeignProcessingUnavailable`) until the user resolves, reinstalls,
/// bypasses, or removes the device.
pub struct UnavailablePackageEffect {
    type_id: DeviceTypeId,
    state_version: u32,
}

impl UnavailablePackageEffect {
    pub fn new(type_id: DeviceTypeId, state_version: u32) -> Self {
        Self {
            type_id,
            state_version,
        }
    }
}

impl NativeDeviceMetadata for UnavailablePackageEffect {
    fn type_id(&self) -> DeviceTypeId {
        self.type_id
    }

    fn state_version(&self) -> u32 {
        self.state_version
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != self.type_id {
            return Err(DeviceContractError::WrongDeviceType);
        }
        // Validate envelope syntax without executing any untrusted codec.
        let _ = PackageIdentityEnvelope::decode_with_raw_state(state.payload())?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
}

impl NativeDeviceDefinition for UnavailablePackageEffect {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn prepare_runtime_factory(
        &self,
        _state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
}

/// Unavailable placeholder instrument adapter (`#2993`). Preserves opaque
/// package state verbatim without executing untrusted codecs, while preparing
/// a visibly silent, allocation-free mutable runtime so unaffected tracks and
/// project editing remain usable.
pub struct UnavailablePackageInstrument {
    type_id: DeviceTypeId,
    state_version: u32,
}

impl UnavailablePackageInstrument {
    pub fn new(type_id: DeviceTypeId, state_version: u32) -> Self {
        Self {
            type_id,
            state_version,
        }
    }
}

impl NativeDeviceMetadata for UnavailablePackageInstrument {
    fn type_id(&self) -> DeviceTypeId {
        self.type_id
    }

    fn state_version(&self) -> u32 {
        self.state_version
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::STEREO_INSTRUMENT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != self.type_id {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let _ = PackageIdentityEnvelope::decode_with_raw_state(state.payload())?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn patch_parameter(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
        _value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
}

impl NativeInstrumentDefinition for UnavailablePackageInstrument {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let _ = self.migrate_state(state)?;
        Ok(Box::new(PreparedUnavailableInstrument))
    }
}

struct PreparedUnavailableInstrument;

impl PreparedNativeInstrument for PreparedUnavailableInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
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
        Ok(Some(Box::new(UnavailableInstrumentFactory)))
    }
}

struct UnavailableInstrumentFactory;

impl PreparedMutableInstrumentRuntime for UnavailableInstrumentFactory {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<SilentUnavailableInstrumentRuntime>()
    }

    fn max_events_per_block(&self) -> usize {
        PACKAGE_ABI_MAX_NOTE_EVENTS as usize
    }

    fn max_active_occurrences(&self) -> usize {
        MAX_PACKAGE_VOICES
    }

    fn max_release_tail_frames(&self) -> u64 {
        0
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(SilentUnavailableInstrumentRuntime)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct SilentUnavailableInstrumentRuntime;

impl MutableInstrumentRuntime for SilentUnavailableInstrumentRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > PACKAGE_ABI_MAX_NOTE_EVENTS as usize {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
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
        _events: &[NativeInstrumentEvent],
        _frames: &mut [StereoFrame],
        _resources: &dyn DeviceResources,
    ) {
        // Visibly unavailable instrument renders silence without mutating state.
    }

    fn reset(&mut self) {}

    fn is_quiescent(&self) -> bool {
        true
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, _previous: &dyn MutableInstrumentRuntime) {}

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Neutral control-side load specification for one admitted installed package
/// module (`#3004`). Carries only verified catalog paths and identity digests —
/// zero hardcoded device types or static device switches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPackageLoadEntry {
    pub dylib_path: std::path::PathBuf,
    pub publisher_id: String,
    pub package_sha256: [u8; 32],
    pub device_kind: u32,
}

/// Generic control-loaded bank of installed `.hexdevice` effect and instrument
/// definitions (`#3004`). Builds ordinary `NativeDeviceRegistry` and
/// `NativeInstrumentRegistry` views purely from admitted on-disk catalog
/// packages with zero hardcoded device-type switches or static fallbacks.
pub struct InstalledPackageRuntimeBank {
    effects: Vec<InstalledPackageEffect>,
    instruments: Vec<InstalledPackageInstrument>,
}

impl InstalledPackageRuntimeBank {
    /// Load all admitted catalog entries on control. Never call on audio callbacks.
    pub fn load_on_control(
        entries: &[InstalledPackageLoadEntry],
    ) -> Result<Self, DeviceContractError> {
        let mut effects = Vec::new();
        let mut instruments = Vec::new();
        for entry in entries {
            match entry.device_kind {
                DEVICE_KIND_AUDIO_EFFECT => {
                    effects.push(InstalledPackageEffect::load_on_control(
                        &entry.dylib_path,
                        entry.publisher_id.clone(),
                        entry.package_sha256,
                    )?);
                }
                DEVICE_KIND_INSTRUMENT => {
                    instruments.push(InstalledPackageInstrument::load_on_control(
                        &entry.dylib_path,
                        entry.publisher_id.clone(),
                        entry.package_sha256,
                    )?);
                }
                _ => return Err(DeviceContractError::WrongDeviceType),
            }
        }
        Ok(Self {
            effects,
            instruments,
        })
    }

    pub fn effects(&self) -> &[InstalledPackageEffect] {
        &self.effects
    }

    pub fn instruments(&self) -> &[InstalledPackageInstrument] {
        &self.instruments
    }

    pub fn effect_refs(&self) -> Vec<&dyn NativeDeviceDefinition> {
        self.effects
            .iter()
            .map(|e| e as &dyn NativeDeviceDefinition)
            .collect()
    }

    pub fn instrument_refs(&self) -> Vec<&dyn NativeInstrumentDefinition> {
        self.instruments
            .iter()
            .map(|i| i as &dyn NativeInstrumentDefinition)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceRegistry,
        NativeInstrumentRegistry, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
        TuningTable,
    };
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hex-installed-dev-{tag}-{}-{}",
                std::process::id(),
                TMP_SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn compile_fixture(tmp: &TempDir, name: &str, file: &str) -> PathBuf {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let src = repo_root
            .join("third_party/hex-device-abi-qual/fixtures")
            .join(file);
        let inc = repo_root.join("third_party/hex-device-abi-qual/include");
        let out = tmp.0.join(format!("lib{name}.dylib"));
        let res = Command::new("clang")
            .args([
                "-std=c99", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
            ])
            .arg("-I")
            .arg(&inc)
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .arg("-lm")
            .output()
            .unwrap();
        assert!(
            res.status.success(),
            "clang failed: {}",
            String::from_utf8_lossy(&res.stderr)
        );
        out
    }

    #[test]
    fn installed_utility_and_sine_run_through_ordinary_lattice_registries_and_oracles() {
        let tmp = TempDir::new("engine_registry");
        let util_dylib = compile_fixture(&tmp, "utility", "utility_v1.c");
        let sine_dylib = compile_fixture(&tmp, "sine", "sine_v1.c");

        let util_def =
            InstalledPackageEffect::load_on_control(&util_dylib, "hex.factory", [0x11; 32])
                .unwrap();
        let sine_def =
            InstalledPackageInstrument::load_on_control(&sine_dylib, "hex.factory", [0x22; 32])
                .unwrap();

        // Register through the exact ordinary NativeDeviceRegistry & NativeInstrumentRegistry!
        let effects_slice: [&dyn NativeDeviceDefinition; 1] = [&util_def];
        let instruments_slice: [&dyn NativeInstrumentDefinition; 1] = [&sine_def];
        let effect_reg = NativeDeviceRegistry::new(&effects_slice).unwrap();
        let inst_reg = NativeInstrumentRegistry::new(&instruments_slice).unwrap();

        let util_id = DeviceTypeId::new("hex.pkg.utility").unwrap();
        let sine_id = DeviceTypeId::new("hex.pkg.sine").unwrap();

        // Patch Utility gain to 2.0 through ordinary registry path and verify readback.
        let util_default = effect_reg.default_state(util_id).unwrap();
        let util_patched = effect_reg
            .patch_parameter(
                &util_default,
                ParameterId::from_static(1),
                ParameterValue::Float(2.0),
            )
            .unwrap();
        assert_eq!(
            util_def
                .read_parameter(&util_patched, ParameterId::from_static(1))
                .unwrap(),
            ParameterValue::Float(2.0)
        );

        // Mismatched package hash refuses nondestructively.
        let other_util_def =
            InstalledPackageEffect::load_on_control(&util_dylib, "hex.factory", [0x99; 32])
                .unwrap();
        assert_eq!(
            other_util_def.migrate_state(&util_patched),
            Err(DeviceContractError::UnsupportedStateVersion)
        );

        // Prepare and render Sine through ordinary NativeInstrumentRuntimePair -> Utility OwnedProcessorRuntime!
        let tuning = TuningTable::default();
        let ctx = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        let sine_state = inst_reg.default_state(sine_id).unwrap();
        let prepared_sine = inst_reg.prepare_instrument(&sine_state, &ctx).unwrap();
        let mutable_factory = prepared_sine.prepare_mutable_runtime().unwrap().unwrap();
        let mut pair = NativeInstrumentRuntimePair::new(
            mutable_factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 512 * 1024,
                max_events_per_block: 64,
                max_active_occurrences: 16,
                max_tail_frames: 48_000,
            },
        )
        .unwrap();

        let mut buf = [[0.0f32; 2]; 128];
        let events = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 10,
                occurrence_id: 1,
                pitch: 69,
                velocity: 127,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 90,
                occurrence_id: 1,
            },
        ];
        pair.begin_block();
        pair.process_block(&events, &mut buf, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
        pair.commit_block();
        assert!(pair.committed_is_quiescent());

        // Independent Sine oracle at 48 kHz, A4 = 440 Hz, velocity 1.0, default gain 0.5.
        assert_eq!(buf[9], [0.0, 0.0]);
        let phase_inc = std::f64::consts::TAU * 440.0 / 48_000.0;
        let expected_11 = (phase_inc.sin() * 0.5) as f32;
        assert!((buf[11][0] - expected_11).abs() < 1e-6);
        assert_eq!(buf[90], [0.0, 0.0]);

        // Now run through installed Utility effect (gain = 2.0).
        let Some(PreparedProcessorFactory::Owned(util_factory)) = util_def
            .prepare_runtime_factory(&util_patched, &ctx)
            .unwrap()
        else {
            panic!("expected owned runtime factory");
        };
        let mut util_rt = util_factory.create_runtime().unwrap();
        let pre_gain_11 = buf[11][0];
        util_rt.begin_block();
        util_rt.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
        util_rt.commit_block();
        assert!((buf[11][0] - pre_gain_11 * 2.0).abs() < 1e-6);

        // Lease count drops cleanly when runtimes drop on control.
        assert!(util_def.active_module_leases() >= 3);
        drop(util_rt);
        drop(util_factory);
        assert_eq!(util_def.active_module_leases(), 1);
    }

    #[test]
    fn unavailable_package_effect_refuses_processing_and_instrument_renders_silence_preserving_state()
     {
        let util_id = DeviceTypeId::new("hex.pkg.utility").unwrap();
        let sine_id = DeviceTypeId::new("hex.pkg.sine").unwrap();
        let env = PackageIdentityEnvelope {
            publisher_id: "hex.factory".to_owned(),
            type_id: "hex.pkg.utility".to_owned(),
            package_major: 1,
            package_minor: 0,
            package_patch: 0,
            state_version: 1,
            abi_version: 1,
            ui_version: 1,
            package_sha256: [0x55; 32],
        };
        let state = NativeDeviceState::new(
            util_id,
            1,
            env.encode_with_raw_state(b"preserved-opaque").unwrap(),
        )
        .unwrap();

        let tuning = TuningTable::default();
        let ctx = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };

        let unavailable_fx = UnavailablePackageEffect::new(util_id, 1);
        assert_eq!(unavailable_fx.migrate_state(&state).unwrap(), state);
        assert_eq!(
            unavailable_fx.prepare_matrix(&state),
            Err(DeviceContractError::ForeignProcessingUnavailable)
        );
        assert!(matches!(
            unavailable_fx.prepare_runtime_factory(&state, &ctx),
            Err(DeviceContractError::ForeignProcessingUnavailable)
        ));

        let sine_env = PackageIdentityEnvelope {
            type_id: "hex.pkg.sine".to_owned(),
            ..env
        };
        let sine_state = NativeDeviceState::new(
            sine_id,
            1,
            sine_env.encode_with_raw_state(b"preserved-sine").unwrap(),
        )
        .unwrap();
        let unavailable_inst = UnavailablePackageInstrument::new(sine_id, 1);
        assert_eq!(
            unavailable_inst.migrate_state(&sine_state).unwrap(),
            sine_state
        );
        let prepared = unavailable_inst
            .prepare_instrument(&sine_state, &ctx)
            .unwrap();
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        let mut rt = factory.create_runtime();
        let mut buf = [[0.0f32; 2]; 32];
        rt.process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 69,
                velocity: 127,
            }],
            &mut buf,
            &EMPTY_DEVICE_RESOURCES,
        );
        assert_eq!(buf, [[0.0; 2]; 32]);
        assert!(rt.is_quiescent());
    }
}
