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

use hex_device_abi::{PackageInstance, PackageModule};
use hex_lattice::{
    DevicePreparationContext, DeviceResources, MutableInstrumentRuntime, NativeDeviceDefinition,
    NativeInstrumentDefinition, NativeInstrumentEvent, NativeInstrumentRuntimeError, OutputTail,
    OwnedProcessorRuntime, PreparedAnalyticNote, PreparedMutableInstrumentRuntime,
    PreparedNativeInstrument, PreparedOwnedProcessorRuntime, PreparedProcessorFactory, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, CAP_STATE_PERSISTENCE,
    DEVICE_KIND_AUDIO_EFFECT, DEVICE_KIND_INSTRUMENT, NOTE_EVENT_OFF, NOTE_EVENT_ON,
    PACKAGE_ABI_MAX_FRAMES, PACKAGE_ABI_MAX_NOTE_EVENTS, PACKAGE_ABI_MAX_STATE_BYTES,
    PackageIdentityEnvelope,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::path::Path;
use std::sync::Arc;

const MAX_PACKAGE_VOICES: usize = 16;
const PARAM_TABLE_MAGIC: [u8; 4] = *b"HXPM";

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
        let (_, raw_state) = self.shared.validate_and_decode(state)?;
        let factory = PreparedInstalledEffect {
            module: self.shared.module.clone(),
            descriptor: self.shared.descriptor,
            sample_rate: context.sample_rate,
            initial_raw_state: raw_state.to_vec(),
        };
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(factory))))
    }
}

struct PreparedInstalledEffect {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: Vec<u8>,
}

impl PreparedOwnedProcessorRuntime for PreparedInstalledEffect {
    fn supports_smoothed_bypass(&self) -> bool {
        self.descriptor.reported_latency_frames == 0
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
        Ok(Box::new(InstalledEffectRuntime {
            committed,
            speculative,
            has_state_persistence: (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0,
            baseline_state,
            baseline_len,
            state_scratch: [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
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
    baseline_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    baseline_len: usize,
    state_scratch: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
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
}

impl OwnedProcessorRuntime for InstalledEffectRuntime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn begin_block(&mut self) {
        Self::sync_instance(
            self.has_state_persistence,
            &self.committed,
            &mut self.speculative,
            &mut self.state_scratch,
        );
    }

    fn process_block(&mut self, frames: &mut [StereoFrame], _resources: &dyn DeviceResources) {
        if self.faulted {
            frames.fill([0.0; 2]);
            return;
        }
        let max_chunk = PACKAGE_ABI_MAX_FRAMES as usize;
        for chunk in frames.chunks_mut(max_chunk) {
            let len = chunk.len();
            if self
                .speculative
                .process(chunk, &mut self.audio_scratch[..len], &[], &[])
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
        Self::sync_instance(
            self.has_state_persistence,
            &self.speculative,
            &mut self.committed,
            &mut self.state_scratch,
        );
    }

    fn rollback_block(&mut self) {
        Self::sync_instance(
            self.has_state_persistence,
            &self.committed,
            &mut self.speculative,
            &mut self.state_scratch,
        );
    }

    fn reset(&mut self) {
        self.faulted = false;
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
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let (_, raw_state) = self.shared.validate_and_decode(state)?;
        Ok(Box::new(PreparedInstalledInstrument {
            module: self.shared.module.clone(),
            descriptor: self.shared.descriptor,
            sample_rate: context.sample_rate,
            initial_raw_state: raw_state.to_vec(),
            identity_digest: self.shared.identity.package_sha256,
        }))
    }
}

struct PreparedInstalledInstrument {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: Vec<u8>,
    identity_digest: [u8; 32],
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
            initial_raw_state: self.initial_raw_state.clone(),
            identity_digest: self.identity_digest,
        })))
    }
}

struct PreparedInstalledInstrumentFactory {
    module: PackageModule,
    descriptor: AbiDescriptor,
    sample_rate: u32,
    initial_raw_state: Vec<u8>,
    identity_digest: [u8; 32],
}

impl PreparedMutableInstrumentRuntime for PreparedInstalledInstrumentFactory {
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
            && !self.initial_raw_state.is_empty()
        {
            let _ = inst.set_state(&self.initial_raw_state);
        }
        let mut baseline_state = [0u8; PACKAGE_ABI_MAX_STATE_BYTES as usize];
        let baseline_len = self.initial_raw_state.len().min(baseline_state.len());
        baseline_state[..baseline_len].copy_from_slice(&self.initial_raw_state[..baseline_len]);
        Box::new(InstalledInstrumentRuntime {
            instance,
            sample_rate: self.sample_rate,
            identity_digest: self.identity_digest,
            has_state_persistence: (self.descriptor.capabilities & CAP_STATE_PERSISTENCE) != 0,
            baseline_state,
            baseline_len,
            active_occurrences: [None; MAX_PACKAGE_VOICES],
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
    identity_digest: [u8; 32],
    has_state_persistence: bool,
    baseline_state: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    baseline_len: usize,
    active_occurrences: [Option<(u64, u8)>; MAX_PACKAGE_VOICES],
    note_scratch: [AbiNoteEvent; PACKAGE_ABI_MAX_NOTE_EVENTS as usize],
    silence_in: [StereoFrame; PACKAGE_ABI_MAX_FRAMES as usize],
    audio_scratch: [StereoFrame; PACKAGE_ABI_MAX_FRAMES as usize],
    state_scratch: [u8; PACKAGE_ABI_MAX_STATE_BYTES as usize],
    faulted: bool,
}

impl MutableInstrumentRuntime for InstalledInstrumentRuntime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
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
                _ => {}
            }
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn DeviceResources,
    ) {
        if self.faulted || frames.is_empty() {
            return;
        }
        let Some(inst) = self.instance.as_mut() else {
            return;
        };
        let max_chunk = PACKAGE_ABI_MAX_FRAMES as usize;
        let mut block_start = 0usize;
        while block_start < frames.len() {
            let chunk_len = (frames.len() - block_start).min(max_chunk);
            let block_end = block_start + chunk_len;
            let mut note_count = 0usize;
            for &ev in events {
                let offset = ev.frame_offset();
                if offset < block_start || offset >= block_end {
                    continue;
                }
                let local_offset = (offset - block_start) as u32;
                match ev {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        velocity,
                        ..
                    } => {
                        if let Some(slot) = self.active_occurrences.iter_mut().find(|s| s.is_none())
                        {
                            *slot = Some((occurrence_id, pitch));
                        }
                        if note_count < self.note_scratch.len() {
                            self.note_scratch[note_count] = AbiNoteEvent {
                                sample_offset: local_offset,
                                kind: NOTE_EVENT_ON,
                                note: pitch,
                                channel: 0,
                                reserved0: 0,
                                velocity: f32::from(velocity) / 127.0,
                                detune_cents: 0.0,
                                reserved1: 0,
                            };
                            note_count += 1;
                        }
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        if let Some(slot) = self
                            .active_occurrences
                            .iter_mut()
                            .find(|s| matches!(s, Some((id, _)) if *id == occurrence_id))
                        {
                            let pitch = slot.map(|(_, p)| p).unwrap_or(0);
                            *slot = None;
                            if note_count < self.note_scratch.len() {
                                self.note_scratch[note_count] = AbiNoteEvent {
                                    sample_offset: local_offset,
                                    kind: NOTE_EVENT_OFF,
                                    note: pitch,
                                    channel: 0,
                                    reserved0: 0,
                                    velocity: 0.0,
                                    detune_cents: 0.0,
                                    reserved1: 0,
                                };
                                note_count += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }

            if inst
                .process(
                    &self.silence_in[..chunk_len],
                    &mut self.audio_scratch[..chunk_len],
                    &[],
                    &self.note_scratch[..note_count],
                )
                .is_err()
            {
                self.faulted = true;
                return;
            }
            for i in 0..chunk_len {
                frames[block_start + i][0] += self.audio_scratch[i][0];
                frames[block_start + i][1] += self.audio_scratch[i][1];
            }
            block_start = block_end;
        }
    }

    fn reset(&mut self) {
        self.faulted = false;
        // Emit note-off for every active note so C module clears voices without reallocating.
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
        self.active_occurrences.iter().all(Option::is_none)
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|src| {
            src.sample_rate == self.sample_rate && src.identity_digest == self.identity_digest
        })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(src) = previous.as_any().downcast_ref::<Self>() {
            self.active_occurrences = src.active_occurrences;
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
