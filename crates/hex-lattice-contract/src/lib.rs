//! # Purpose
//! Own neutral bounded native-device metadata, identifiers and authored state envelopes.
//! Also owns shared foreign-processing bindings and copied parameter data;
//! neither grants native custody.
//!
//! # Dependencies
//! Uses only the standard library; no DSP or other workspace dependency.
//!
//! # Does not own
//! Does not construct processors, resolve assets, schedule notes or perform storage migrations.
//! Opaque payloads and asset references require provider and host validation before processing.
mod automation_target;
mod descriptor_validation;
pub use descriptor_validation::validate_parameter_descriptors;
mod device_assets;
pub use automation_target::DeviceAutomationTarget;
mod host_gain;
mod modulation_source_id;
pub use host_gain::HostGain;
pub use modulation_source_id::ModulationSourceId;
mod foreign_binding;
mod foreign_parameter;
mod vst3_state_approval;
pub use foreign_binding::ForeignProcessorBinding;
pub use foreign_parameter::ForeignParameterPoint;
pub use vst3_state_approval::{Vst3StateAuthority, Vst3StateVerifier};
#[cfg(test)]
mod device_assets_tests;
pub use device_assets::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES,
};
mod parameter_semantics;
#[cfg(test)]
mod parameter_semantics_tests;
#[cfg(test)]
mod snapshot_tests;
pub use parameter_semantics::*;
use std::{fmt, sync::Arc};

/// Maximum opaque state for providers without a dedicated bound.
pub const MAX_NATIVE_DEVICE_STATE_BYTES: usize = 4_096;
/// Maximum complete opaque state envelope for the VST3 provider.
pub const MAX_VST3_DEVICE_STATE_BYTES: usize = 512 * 1024;
pub const VST3_DEVICE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");
pub const MAX_DEVICE_TYPE_ID_BYTES: usize = 128;
/// Device-owned IDs exclude zero and the two host gain identifiers.
/// Devices deriving IDs from stable object identities must use checked
/// arithmetic and refuse exhaustion instead of wrapping/reusing an ID.
pub const MAX_PARAMETER_ID: u32 = u32::MAX - 2;
pub const MAX_NATIVE_DEVICE_TYPES: usize = 256;
pub const MAX_PARAMETERS_PER_DEVICE: usize = 512;
pub const MAX_PARAMETER_SNAPSHOT_ENTRIES: usize = 64;

/// Control-side atomic edits. Unused slots are canonical and cannot be supplied
/// by callers; the callback consumes prepared device laws, never this payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceParameterSnapshot {
    entries: [(ParameterId, ParameterValue); MAX_PARAMETER_SNAPSHOT_ENTRIES],
    count: u8,
}

impl DeviceParameterSnapshot {
    pub fn new(entries: &[(ParameterId, ParameterValue)]) -> Result<Self, DeviceContractError> {
        if entries.is_empty() || entries.len() > MAX_PARAMETER_SNAPSHOT_ENTRIES {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut snapshot = Self {
            entries: [(ParameterId::from_static(1), ParameterValue::Float(0.0));
                MAX_PARAMETER_SNAPSHOT_ENTRIES],
            count: entries.len() as u8,
        };
        for (index, &(id, value)) in entries.iter().enumerate() {
            id.validate()?;
            if entries[..index].iter().any(|entry| entry.0 == id)
                || matches!(value, ParameterValue::Float(value) if !value.is_finite())
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            snapshot.entries[index] = (id, value);
        }
        Ok(snapshot)
    }

    pub fn entries(&self) -> &[(ParameterId, ParameterValue)] {
        &self.entries[..usize::from(self.count)]
    }

    pub fn same_parameters(&self, other: &Self) -> bool {
        self.count == other.count
            && self
                .entries()
                .iter()
                .zip(other.entries())
                .all(|(a, b)| a.0 == b.0)
    }

    pub fn validate(&self, descriptors: &[ParameterDescriptor]) -> Result<(), DeviceContractError> {
        for &(id, value) in self.entries() {
            descriptors
                .iter()
                .find(|descriptor| descriptor.id == id)
                .ok_or(DeviceContractError::UnknownParameter)?
                .validate(value)?;
        }
        Ok(())
    }
}
/// Maximum UTF-8 bytes in a parameter label, unit or choice label.
pub const MAX_PARAMETER_LABEL_BYTES: usize = 128;
/// Maximum alternatives exposed by one native parameter selector.
pub const MAX_PARAMETER_CHOICES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceTypeId {
    bytes: [u8; MAX_DEVICE_TYPE_ID_BYTES],
    length: u8,
    valid: bool,
}
impl DeviceTypeId {
    pub fn new(value: &str) -> Result<Self, DeviceContractError> {
        if !Self::is_valid(value) {
            return Err(DeviceContractError::InvalidId);
        }
        let mut bytes = [0; MAX_DEVICE_TYPE_ID_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            length: value.len() as u8,
            valid: true,
        })
    }
    /// Static built-in identifier; state and registry boundaries still validate it.
    pub const fn from_static(value: &str) -> Self {
        let source = value.as_bytes();
        let mut bytes = [0; MAX_DEVICE_TYPE_ID_BYTES];
        let mut index = 0;
        while index < source.len() && index < MAX_DEVICE_TYPE_ID_BYTES {
            bytes[index] = source[index];
            index += 1;
        }
        Self {
            bytes,
            length: if source.len() <= MAX_DEVICE_TYPE_ID_BYTES {
                source.len() as u8
            } else {
                0
            },
            valid: source.len() <= MAX_DEVICE_TYPE_ID_BYTES,
        }
    }
    pub fn as_str(&self) -> Result<&str, DeviceContractError> {
        if !self.valid {
            return Err(DeviceContractError::InvalidId);
        }
        std::str::from_utf8(&self.bytes[..usize::from(self.length)])
            .map_err(|_| DeviceContractError::InvalidId)
    }
    pub fn validate(self) -> Result<(), DeviceContractError> {
        Self::is_valid(self.as_str()?)
            .then_some(())
            .ok_or(DeviceContractError::InvalidId)
    }
    fn is_valid(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= MAX_DEVICE_TYPE_ID_BYTES
            && value.contains('.')
            && !value.starts_with('.')
            && !value.ends_with('.')
            && !value.contains("..")
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterId(u32);
impl ParameterId {
    pub const fn new(value: u32) -> Result<Self, DeviceContractError> {
        if value == 0 || value > MAX_PARAMETER_ID {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }
    /// Const constructor for statically declared built-in parameter identifiers.
    pub const fn from_static(value: u32) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u32 {
        self.0
    }
    pub const fn validate(self) -> Result<(), DeviceContractError> {
        if self.0 == 0 || self.0 > MAX_PARAMETER_ID {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterValue {
    Float(f32),
    Bool(bool),
    Choice(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterKind {
    Float {
        min: f32,
        max: f32,
        /// Unit of the canonical value and range, before any presentation mapping.
        unit: Option<&'static str>,
    },
    Bool,
    Choice {
        labels: &'static [&'static str],
    },
}

/// How a parameter's canonical value is presented to generic controls.
/// This changes UI input/display only; descriptors, edits and persisted values
/// remain in the domain and unit declared by [`ParameterKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterPresentation {
    /// Display and edit the canonical value directly.
    #[default]
    Linear,
    /// Canonical positive values are edited and presented on a logarithmic scale.
    Logarithmic,
    /// Canonical values are linear amplitudes presented and entered in dB.
    LinearAmplitudeDecibels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterDescriptor {
    pub id: ParameterId,
    pub label: &'static str,
    pub kind: ParameterKind,
    pub default: ParameterValue,
    pub presentation: ParameterPresentation,
}
impl ParameterDescriptor {
    pub fn validate_descriptor(self) -> Result<(), DeviceContractError> {
        self.id.validate()?;
        if !valid_parameter_text(self.label) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if self.presentation == ParameterPresentation::LinearAmplitudeDecibels
            && !matches!(self.kind, ParameterKind::Float { min, max, .. } if min >= 0.0 && max > min)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if self.presentation == ParameterPresentation::Logarithmic
            && !matches!(self.kind, ParameterKind::Float { min, max, .. } if min > 0.0 && max > min)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        match self.kind {
            ParameterKind::Float { min, max, unit }
                if !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || unit.is_some_and(|value| !valid_parameter_text(value)) =>
            {
                Err(DeviceContractError::InvalidDescriptor)
            }
            ParameterKind::Choice { labels }
                if labels.is_empty()
                    || labels.len() > MAX_PARAMETER_CHOICES
                    || labels.iter().any(|label| !valid_parameter_text(label)) =>
            {
                Err(DeviceContractError::InvalidDescriptor)
            }
            _ => self.validate(self.default),
        }
    }
    pub fn validate(self, value: ParameterValue) -> Result<(), DeviceContractError> {
        let valid = match (self.kind, value, self.default) {
            (ParameterKind::Float { min, max, .. }, ParameterValue::Float(v), _) => {
                v.is_finite() && v >= min && v <= max
            }
            (ParameterKind::Bool, ParameterValue::Bool(_), _) => true,
            (ParameterKind::Choice { labels }, ParameterValue::Choice(v), _) => {
                !labels.is_empty() && (v as usize) < labels.len()
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(DeviceContractError::InvalidParameterValue)
        }
    }
}

// Metadata visible text and selector work must still
// satisfy the shared display contract before either registry admits a device.
fn valid_parameter_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PARAMETER_LABEL_BYTES
        && !value
            .chars()
            .any(|character| character.is_uppercase() || character.is_control())
}

#[derive(Debug, Clone)]
pub struct NativeDeviceState {
    type_id: DeviceTypeId,
    state_version: u32,
    payload: Arc<[u8]>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
    asset_references: Option<Arc<[DeviceAssetReference]>>,
    // Process-local source provenance, never canonical or serialized state.
    source_approval: Option<std::num::NonZeroU64>,
    processing_approval: Option<std::num::NonZeroU64>,
}
impl PartialEq for NativeDeviceState {
    fn eq(&self, other: &Self) -> bool {
        self.processing_eq(other)
            && self.input_gain_millidb == other.input_gain_millidb
            && self.output_gain_millidb == other.output_gain_millidb
    }
}
impl Eq for NativeDeviceState {}
impl NativeDeviceState {
    pub fn new(
        type_id: DeviceTypeId,
        state_version: u32,
        payload: Vec<u8>,
    ) -> Result<Self, DeviceContractError> {
        type_id.validate()?;
        if state_version == 0 {
            return Err(DeviceContractError::InvalidStateVersion);
        }
        let max_bytes = if type_id == VST3_DEVICE_TYPE_ID {
            MAX_VST3_DEVICE_STATE_BYTES
        } else {
            MAX_NATIVE_DEVICE_STATE_BYTES
        };
        if payload.len() > max_bytes {
            return Err(DeviceContractError::StateTooLarge);
        }
        Ok(Self {
            type_id,
            state_version,
            payload: Arc::from(payload),
            input_gain_millidb: 0,
            output_gain_millidb: 0,
            asset_references: None,
            source_approval: None,
            processing_approval: None,
        })
    }
    pub const fn type_id(&self) -> DeviceTypeId {
        self.type_id
    }
    pub const fn state_version(&self) -> u32 {
        self.state_version
    }
    /// Canonical slot order; empty states have no reference-list allocation.
    pub fn asset_references(&self) -> &[DeviceAssetReference] {
        self.asset_references.as_deref().unwrap_or(&[])
    }
    /// Bind host-owned project identities without changing the DSP payload.
    /// Resolution and file/resource preparation remain separate host operations.
    pub fn with_asset_references(
        mut self,
        mut references: Vec<DeviceAssetReference>,
    ) -> Result<Self, DeviceContractError> {
        if references.len() > MAX_DEVICE_ASSET_REFERENCES {
            return Err(DeviceContractError::StateTooLarge);
        }
        self.source_approval = None;
        self.processing_approval = None;
        references.sort_unstable_by_key(|reference| reference.slot);
        if references
            .windows(2)
            .any(|pair| pair[0].slot == pair[1].slot)
        {
            return Err(DeviceContractError::MalformedState);
        }
        self.asset_references = if references.is_empty() {
            None
        } else {
            Some(Arc::from(references))
        };
        Ok(self)
    }
    /// Restore host metadata after a provider reconstructs its opaque state.
    /// Explicit host binding edits use `with_asset_references` instead.
    pub fn with_host_metadata_from(mut self, source: &Self) -> Result<Self, DeviceContractError> {
        if self.type_id != source.type_id {
            return Err(DeviceContractError::WrongDeviceType);
        }
        self.input_gain_millidb = source.input_gain_millidb;
        self.output_gain_millidb = source.output_gain_millidb;
        if self.asset_references != source.asset_references {
            self.source_approval = None;
            self.processing_approval = None;
        }
        self.asset_references.clone_from(&source.asset_references);
        Ok(self)
    }
    /// Supported runtime admission remains asset-free until resource leases exist.
    /// Check the supplied state before a codec can reconstruct it without bindings.
    pub fn require_asset_free_processing(&self) -> Result<(), DeviceContractError> {
        if self.asset_references.is_some() {
            Err(DeviceContractError::AssetResourcesUnavailable)
        } else {
            Ok(())
        }
    }
    /// Host-owned input trim, independent of the device's opaque DSP payload.
    pub const fn input_gain_millidb(&self) -> i32 {
        self.input_gain_millidb
    }
    pub fn input_gain_db(&self) -> f32 {
        HostGain(self.input_gain_millidb).db()
    }
    pub fn with_input_gain_millidb(mut self, gain: i32) -> Result<Self, DeviceContractError> {
        self.input_gain_millidb = HostGain::from_millidb(gain)?.millidb();
        Ok(self)
    }
    pub fn with_input_gain_db(self, gain: f32) -> Result<Self, DeviceContractError> {
        self.with_input_gain_millidb(HostGain::from_db(gain)?.millidb())
    }
    /// Host-owned output trim, independent of the device's opaque DSP payload.
    pub const fn output_gain_millidb(&self) -> i32 {
        self.output_gain_millidb
    }
    pub fn output_gain_db(&self) -> f32 {
        HostGain(self.output_gain_millidb).db()
    }
    pub fn with_output_gain_millidb(mut self, gain: i32) -> Result<Self, DeviceContractError> {
        self.output_gain_millidb = HostGain::from_millidb(gain)?.millidb();
        Ok(self)
    }
    pub fn with_output_gain_db(self, gain: f32) -> Result<Self, DeviceContractError> {
        self.with_output_gain_millidb(HostGain::from_db(gain)?.millidb())
    }
    pub fn without_host_gains(mut self) -> Self {
        self.input_gain_millidb = 0;
        self.output_gain_millidb = 0;
        self
    }
    /// DSP configuration identity excludes independent host gain staging.
    pub fn processing_eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.state_version == other.state_version
            && (self.shares_payload_storage_with(other) || self.payload == other.payload)
            && self.asset_references == other.asset_references
    }
    /// Match shared immutable payloads on the callback. Independently
    /// allocated larger payloads require a fresh runtime handoff so callback
    /// comparison work remains bounded by the ordinary device state limit.
    pub fn processing_eq_on_callback(&self, other: &Self) -> bool {
        (self.shares_payload_storage_with(other)
            || (self.payload.len() <= MAX_NATIVE_DEVICE_STATE_BYTES
                && other.payload.len() <= MAX_NATIVE_DEVICE_STATE_BYTES))
            && self.processing_eq(other)
    }

    pub fn input_gain_linear(&self) -> f32 {
        HostGain(self.input_gain_millidb).linear()
    }
    pub fn output_gain_linear(&self) -> f32 {
        HostGain(self.output_gain_millidb).linear()
    }
    /// Physical observations of this state's immutable payload handle. These
    /// counts/bytes establish neither constructor provenance nor lifetime
    /// closure. The payload Arc itself never escapes.
    pub fn payload_storage_observation(&self) -> (usize, usize, usize) {
        let Self {
            type_id: _,
            state_version: _,
            payload,
            input_gain_millidb: _,
            output_gain_millidb: _,
            asset_references: _,
            source_approval: _,
            processing_approval: _,
        } = self;
        (
            payload.len(),
            Arc::strong_count(payload),
            Arc::weak_count(payload),
        )
    }
    /// Actual shared backing identity, independently of equal state bytes.
    pub fn shares_payload_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.payload, &other.payload)
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Conservative shared payload/reference backing retained by this control-side state.
    /// The inline state is charged by its enclosing owner. Arc headers/alignment are included;
    /// reference entries contain only inline identities. Shared clones may be charged repeatedly,
    /// so this is neither allocation identity nor an exclusive/closed-ownership proof.
    pub fn retained_control_metadata_bytes(&self) -> Option<usize> {
        let header = 2usize.checked_mul(std::mem::size_of::<usize>())?;
        let payload = self.payload.len().checked_add(header)?.checked_add(
            std::mem::align_of::<usize>()
                .max(std::mem::align_of::<u8>())
                .checked_sub(1)?,
        )?;
        let references = match self.asset_references.as_ref() {
            Some(references) => references
                .len()
                .checked_mul(std::mem::size_of::<DeviceAssetReference>())?
                .checked_add(header)?
                .checked_add(
                    std::mem::align_of::<usize>()
                        .max(std::mem::align_of::<DeviceAssetReference>())
                        .checked_sub(1)?,
                )?,
            None => 0,
        };
        payload.checked_add(references)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceContractError {
    InvalidId,
    InvalidStateVersion,
    StateTooLarge,
    WrongDeviceType,
    UnsupportedStateVersion,
    MalformedState,
    UnknownParameter,
    InvalidParameterValue,
    InvalidDescriptor,
    DuplicateDeviceType,
    RuntimeBudgetExceeded,
    RuntimeAllocation,
    UnsupportedProcessingClass,
    AssetResourcesUnavailable,
    ForeignProcessingUnavailable,
    ForeignProcessingFailed,
}
impl fmt::Display for DeviceContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::InvalidId => "device contract id is outside its valid range",
                Self::InvalidStateVersion => "device state version must be non-zero",
                Self::StateTooLarge => "native device state exceeds its size limit",
                Self::WrongDeviceType => "device state has the wrong type",
                Self::UnsupportedStateVersion => "device state version is unsupported",
                Self::MalformedState => "native device state is malformed",
                Self::UnknownParameter => "device parameter is unknown",
                Self::InvalidParameterValue => "device parameter value is invalid",
                Self::InvalidDescriptor => "native device parameter descriptor is invalid",
                Self::DuplicateDeviceType => "native device registry contains a duplicate type",
                Self::RuntimeBudgetExceeded =>
                    "native processor runtime exceeds its declared budget",
                Self::RuntimeAllocation => "native processor runtime allocation failed",
                Self::UnsupportedProcessingClass =>
                    "native device processing class is not admitted",
                Self::ForeignProcessingUnavailable =>
                    "foreign processing resources are unavailable for this graph",
                Self::ForeignProcessingFailed => "foreign processor exchange failed",
                Self::AssetResourcesUnavailable => "native device asset resources are not prepared",
            }
        )
    }
}
impl std::error::Error for DeviceContractError {}

/// Enumerate and validate state-owned descriptors on the control side. Callers
/// must use this boundary even when a definition overrides enumeration.
/// State migration/codec validation belongs to the caller; enumeration must not
/// call registry migration or construct defaults recursively.
pub fn validated_state_descriptors<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
    state.type_id().validate()?;
    if state.type_id() != definition.type_id() {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != definition.state_version() {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let descriptors = definition.descriptors_for_state(state)?;
    validate_parameter_descriptors(&descriptors)?;
    Ok(descriptors)
}

/// Per-definition optional auxiliary input; routing and buffer custody remain
/// host-owned. An undeclared input is never inferred from device type or state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuxiliaryInput {
    #[default]
    None,
    Stereo,
}

/// Device metadata/state operations; implementations own their codec.
pub trait NativeDeviceMetadata: Sync {
    fn type_id(&self) -> DeviceTypeId;
    /// One optional stereo key input, or none for existing single-bus devices.
    fn auxiliary_input(&self) -> AuxiliaryInput {
        AuxiliaryInput::None
    }
    /// Optional lowercase presentation name. The type identifier remains the
    /// stable storage/registry identity; unnamed definitions display that ID.
    fn display_name(&self) -> Option<&'static str> {
        None
    }
    fn state_version(&self) -> u32;
    /// Static catalog descriptors; per-instance consumers use the validated
    /// state descriptor boundary instead.
    fn descriptors(&self) -> &'static [ParameterDescriptor];
    /// Owned descriptors for a current, codec-validated state. Dynamic devices
    /// may override this; consumers validate the returned list independently.
    /// This must not migrate through the registry or construct default state.
    fn descriptors_for_state(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        Ok(self.descriptors().to_vec())
    }
    /// Optional current-state semantics. None is undeclared, not an inferred
    /// capability. Consumers use validated_parameter_semantics/state_parameters.
    /// A declaring provider must reject writes to read-only parameters in its
    /// patch methods; registries also enforce this before calling overrides.
    fn parameter_semantics_for_state(
        &self,
        _state: &NativeDeviceState,
        _parameter: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        Ok(None)
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError>;
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError>;
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError>;
    /// Whether a parameter is currently editable for this validated device state.
    /// This is an editor affordance only: patch validation remains authoritative,
    /// and inactive authored values may stay stored and round-trip unchanged.
    /// Definitions with no conditional controls inherit the always-enabled rule.
    /// Implementations must refuse malformed states and unknown parameters.
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let migrated = self.migrate_state(state)?.with_host_metadata_from(state)?;
        let descriptors = validated_state_descriptors(self, &migrated)?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        let value = self.read_parameter(&migrated, parameter)?;
        descriptor.validate(value)?;
        Ok(true)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError>;

    /// Full validation precedes private patching. Devices with compound laws
    /// may override this to decode/encode once and adopt modes atomically.
    fn patch_parameters(
        &self,
        state: &NativeDeviceState,
        snapshot: &DeviceParameterSnapshot,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut candidate = self.migrate_state(state)?.with_host_metadata_from(state)?;
        validate_parameter_snapshot_for_write(self, &candidate, snapshot)?;
        for &(parameter, value) in snapshot.entries() {
            validate_parameter_write(self, &candidate, parameter, value)?;
            candidate = self
                .patch_parameter(&candidate, parameter, value)?
                .with_host_metadata_from(state)?;
        }
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABILITY_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.availability");
    const AVAILABILITY_PARAMETER: ParameterId = ParameterId::from_static(1);
    const AVAILABILITY_DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
        id: AVAILABILITY_PARAMETER,
        label: "level",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.5),
        presentation: ParameterPresentation::Linear,
    }];
    const MODE_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.mode");
    const MODE_PARAMETER: ParameterId = ParameterId::from_static(1);
    const CONDITIONAL_PARAMETER: ParameterId = ParameterId::from_static(2);
    const MODE_DESCRIPTORS: &[ParameterDescriptor] = &[
        ParameterDescriptor {
            id: MODE_PARAMETER,
            label: "mode",
            kind: ParameterKind::Choice {
                labels: &["off", "on"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: CONDITIONAL_PARAMETER,
            label: "amount",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
    ];

    struct AlwaysEnabledDefinition;
    struct ModeConditionalDefinition;

    impl NativeDeviceMetadata for AlwaysEnabledDefinition {
        fn type_id(&self) -> DeviceTypeId {
            AVAILABILITY_TYPE
        }

        fn state_version(&self) -> u32 {
            1
        }

        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            AVAILABILITY_DESCRIPTORS
        }

        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(AVAILABILITY_TYPE, 1, vec![])
        }

        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != AVAILABILITY_TYPE {
                return Err(DeviceContractError::WrongDeviceType);
            }
            if state.state_version() != 1 {
                return Err(DeviceContractError::UnsupportedStateVersion);
            }
            if !state.payload().is_empty() {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(state.clone())
        }

        fn read_parameter(
            &self,
            _: &NativeDeviceState,
            parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            if parameter == AVAILABILITY_PARAMETER {
                Ok(ParameterValue::Float(0.5))
            } else {
                Err(DeviceContractError::UnknownParameter)
            }
        }

        fn patch_parameter(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
            value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            let descriptor = AVAILABILITY_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.id == parameter)
                .ok_or(DeviceContractError::UnknownParameter)?;
            descriptor.validate(value)?;
            self.migrate_state(state)
        }
    }

    impl NativeDeviceMetadata for ModeConditionalDefinition {
        fn type_id(&self) -> DeviceTypeId {
            MODE_TYPE
        }

        fn state_version(&self) -> u32 {
            1
        }

        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            MODE_DESCRIPTORS
        }

        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(MODE_TYPE, 1, vec![0])
        }

        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != MODE_TYPE {
                return Err(DeviceContractError::WrongDeviceType);
            }
            if state.state_version() != 1 {
                return Err(DeviceContractError::UnsupportedStateVersion);
            }
            if state.payload().len() != 1 || state.payload()[0] > 1 {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(state.clone())
        }

        fn read_parameter(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            self.migrate_state(state)?;
            match parameter {
                MODE_PARAMETER => Ok(ParameterValue::Choice(u32::from(state.payload()[0]))),
                CONDITIONAL_PARAMETER => Ok(ParameterValue::Float(0.5)),
                _ => Err(DeviceContractError::UnknownParameter),
            }
        }

        fn parameter_enabled(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
        ) -> Result<bool, DeviceContractError> {
            let migrated = self.migrate_state(state)?;
            let descriptor = MODE_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.id == parameter)
                .ok_or(DeviceContractError::UnknownParameter)?;
            descriptor.validate(self.read_parameter(&migrated, parameter)?)?;
            Ok(parameter != CONDITIONAL_PARAMETER || migrated.payload()[0] == 1)
        }

        fn patch_parameter(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
            value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            let descriptor = MODE_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.id == parameter)
                .ok_or(DeviceContractError::UnknownParameter)?;
            descriptor.validate(value)?;
            self.migrate_state(state)
        }
    }

    #[test]
    fn state_metadata_quote_retains_shared_odd_payload_and_reference_backing() {
        let state = NativeDeviceState::new(AVAILABILITY_TYPE, 1, vec![7; 3]).unwrap();
        let clone = state.clone();
        assert_eq!(
            state.retained_control_metadata_bytes(),
            clone.retained_control_metadata_bytes()
        );
        let alignment = std::mem::align_of::<usize>();
        let minimum = (3 + 2 * std::mem::size_of::<usize>()).next_multiple_of(alignment);
        assert!(state.retained_control_metadata_bytes().unwrap() >= minimum);
        let bound = state
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(2).unwrap(),
            }])
            .unwrap();
        assert!(
            bound.retained_control_metadata_bytes().unwrap()
                >= clone.retained_control_metadata_bytes().unwrap()
                    + std::mem::size_of::<DeviceAssetReference>()
                    + 2 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn metadata_defaults_valid_parameters_to_enabled_and_validates_state() {
        let definition = AlwaysEnabledDefinition;
        let state = definition.default_state().unwrap();
        assert!(
            definition
                .parameter_enabled(&state, AVAILABILITY_PARAMETER)
                .unwrap()
        );
        assert_eq!(
            definition.parameter_enabled(&state, ParameterId::from_static(2)),
            Err(DeviceContractError::UnknownParameter)
        );
        let malformed = NativeDeviceState::new(AVAILABILITY_TYPE, 1, vec![0]).unwrap();
        assert_eq!(
            definition.parameter_enabled(&malformed, AVAILABILITY_PARAMETER),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn independent_metadata_can_disable_a_parameter_from_its_current_state() {
        let definition = ModeConditionalDefinition;
        let off = definition.default_state().unwrap();
        let on = NativeDeviceState::new(MODE_TYPE, 1, vec![1]).unwrap();
        assert!(
            !definition
                .parameter_enabled(&off, CONDITIONAL_PARAMETER)
                .unwrap()
        );
        assert!(
            definition
                .parameter_enabled(&on, CONDITIONAL_PARAMETER)
                .unwrap()
        );
        assert!(definition.parameter_enabled(&off, MODE_PARAMETER).unwrap());
    }

    #[test]
    fn parameter_ids_preserve_the_full_device_domain_and_refuse_host_ids() {
        for value in [1, 65_535, 65_536, MAX_PARAMETER_ID] {
            assert_eq!(ParameterId::new(value).unwrap().value(), value);
            assert!(ParameterId::from_static(value).validate().is_ok());
        }
        for value in [0, u32::MAX - 1, u32::MAX] {
            assert!(ParameterId::new(value).is_err());
            assert!(ParameterId::from_static(value).validate().is_err());
        }
    }

    #[test]
    fn namespaced_ids_state_versions_and_payloads_are_bounded() {
        assert!(DeviceTypeId::new("hex.utility").is_ok());
        for bad in [
            "",
            "utility",
            ".vendor.device",
            "vendor.device.",
            "vendor..device",
            "Vendor.device",
            "vendor.device/other",
        ] {
            assert!(
                DeviceTypeId::new(bad).is_err(),
                "accepted invalid id {bad:?}"
            );
        }
        assert!(DeviceTypeId::from_static("Bad.ID").validate().is_err());
        let id = DeviceTypeId::new("hex.utility").unwrap();
        assert!(NativeDeviceState::new(id, 0, vec![]).is_err());
        assert!(NativeDeviceState::new(id, 1, vec![0; MAX_NATIVE_DEVICE_STATE_BYTES + 1]).is_err());
        let state = NativeDeviceState::new(id, 1, vec![1, 2]).unwrap();
        let clone = state.clone();
        assert!(Arc::ptr_eq(&state.payload, &clone.payload));
    }

    #[test]
    fn callback_compares_large_vst3_state_only_when_it_shares_payload() {
        let make_state =
            |len| NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![42; len]).unwrap();
        let large = make_state(150_545);
        let independent = make_state(150_545);
        assert!(large.processing_eq(&independent));
        assert!(large.processing_eq_on_callback(&large.clone()));
        assert!(!large.processing_eq_on_callback(&independent));
        assert!(make_state(64).processing_eq_on_callback(&make_state(64)));
    }

    #[test]
    fn vst3_envelopes_have_a_separate_bounded_size() {
        let max = MAX_VST3_DEVICE_STATE_BYTES;
        let state = NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![255; max]).unwrap();
        assert_eq!(state.payload().len(), max);
        assert_eq!(
            NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![0; max + 1]),
            Err(DeviceContractError::StateTooLarge),
        );
    }

    #[test]
    fn descriptor_text_and_choices_are_bounded_lowercase_display_metadata() {
        let descriptor = ParameterDescriptor {
            id: ParameterId::from_static(1),
            label: "level",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        };
        let limit_text: &'static str =
            Box::leak("é".repeat(MAX_PARAMETER_LABEL_BYTES / 2).into_boxed_str());
        let oversized: &'static str =
            Box::leak("x".repeat(MAX_PARAMETER_LABEL_BYTES + 1).into_boxed_str());
        for text in ["", "Öutput", "line\nbreak", "null\0label", oversized] {
            assert_eq!(
                ParameterDescriptor {
                    label: text,
                    ..descriptor
                }
                .validate_descriptor(),
                Err(DeviceContractError::InvalidDescriptor),
                "label {text:?}"
            );
            assert_eq!(
                ParameterDescriptor {
                    kind: ParameterKind::Float {
                        min: 0.0,
                        max: 1.0,
                        unit: Some(text)
                    },
                    ..descriptor
                }
                .validate_descriptor(),
                Err(DeviceContractError::InvalidDescriptor),
                "unit {text:?}"
            );
            assert_eq!(
                ParameterDescriptor {
                    kind: ParameterKind::Choice {
                        labels: Box::leak(vec![text].into_boxed_slice())
                    },
                    default: ParameterValue::Choice(0),
                    ..descriptor
                }
                .validate_descriptor(),
                Err(DeviceContractError::InvalidDescriptor),
                "choice {text:?}"
            );
        }
        assert!(
            ParameterDescriptor {
                label: limit_text,
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 1.0,
                    unit: Some(limit_text)
                },
                ..descriptor
            }
            .validate_descriptor()
            .is_ok()
        );
        let choices: &'static [&'static str] =
            Box::leak(vec![limit_text; MAX_PARAMETER_CHOICES].into_boxed_slice());
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Choice { labels: choices },
                default: ParameterValue::Choice((MAX_PARAMETER_CHOICES - 1) as u32),
                ..descriptor
            }
            .validate_descriptor()
            .is_ok()
        );
        let excessive: &'static [&'static str] =
            Box::leak(vec!["choice"; MAX_PARAMETER_CHOICES + 1].into_boxed_slice());
        assert_eq!(
            ParameterDescriptor {
                kind: ParameterKind::Choice { labels: excessive },
                default: ParameterValue::Choice(0),
                ..descriptor
            }
            .validate_descriptor(),
            Err(DeviceContractError::InvalidDescriptor)
        );
    }

    #[test]
    fn descriptors_reject_bad_ranges_defaults_choices_and_case() {
        let id = ParameterId::new(1).unwrap();
        let good = ParameterDescriptor {
            id,
            label: "gain",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 2.0,
                unit: Some("linear"),
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Linear,
        };
        assert!(good.validate_descriptor().is_ok());
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Float {
                    min: f32::NAN,
                    max: 1.0,
                    unit: None
                },
                ..good
            }
            .validate_descriptor()
            .is_err()
        );

        assert!(
            ParameterDescriptor {
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_ok()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Float {
                    min: -0.1,
                    max: 2.0,
                    unit: Some("db"),
                },
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Bool,
                presentation: ParameterPresentation::LinearAmplitudeDecibels,
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                default: ParameterValue::Float(3.0),
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                label: "Gain",
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
        assert!(
            ParameterDescriptor {
                kind: ParameterKind::Choice {
                    labels: &["a", "b"]
                },
                default: ParameterValue::Choice(2),
                ..good
            }
            .validate_descriptor()
            .is_err()
        );
    }
}
