//! Bounded native Drum Rack for one retained native source.
//!
//! Routes a single host MIDI event stream (`NativeInstrumentEvent` with stable
//! `occurrence_id`s, paired note-offs, MPE, and shared controller events) across
//! up to `DRUM_RACK_MAX_PADS` independent native pad instruments (including
//! sample-backed `Sampler` pads) prepared through the existing
//! `NativeInstrumentRegistry` and host `DeviceResources` custody.
//!
//! # Contract & Semantics
//! - **Pad identity & mapping**: Each pad owns a unique non-zero `pad_id`,
//!   unique non-zero `device_id`, bounded non-empty display `name`, unique
//!   incoming `receive_note` (`0..=127`), outgoing `play_note` (`0..=127`),
//!   optional `choke_group` (`1..=DRUM_RACK_MAX_CHOKE_GROUPS`), linear `gain`,
//!   `muted` flag, and optional child `NativeDeviceState`.
//! - **Same-pitch overlap**: Overlapping `NoteOn`s on the same `receive_note`
//!   carry distinct `occurrence_id`s and route independently to the matched pad;
//!   each `NoteOff` targets only its matching `occurrence_id`.
//! - **Choke groups**: When a pad in choke group `g` receives a `NoteOn`, every
//!   currently held occurrence on *other* active pads in the same choke group `g`
//!   receives an immediate `NoteOff` at the exact same `frame_offset`. Its
//!   routed entry is marked released so a subsequent clip `NoteOff` never
//!   produces an orphaned note-off or stuck note.
//! - **Empty / muted / unmapped pads**: `NoteOn` for an unmapped pitch, empty
//!   pad, or muted pad is ignored; its paired `NoteOff` is ignored cleanly.
//! - **Velocity**: Incoming `velocity` is preserved verbatim to the matched pad.
//! - **Asset custody**: Child `DeviceAssetReference`s are aggregated onto the
//!   outer Drum Rack `NativeDeviceState` with deterministic non-zero outer
//!   `DeviceAssetSlot`s (`1..=N`) and remapped back to each pad's original child
//!   slots through a zero-allocation stack `DeviceResources` view. No parallel
//!   sample storage owner is created; missing assets fail preparation with
//!   `DeviceContractError::AssetResourcesUnavailable`.
//! - **Nesting**: Nested Drum Racks and Instrument Racks are refused explicitly
//!   (`UnsupportedProcessingClass`).
//! - **Real-time safety**: All child runtimes, delay rings, and event scratch
//!   buffers are allocated on control. Callback processing performs zero heap
//!   allocations, locks, I/O, or final drops.

#[cfg(test)]
mod tests;

use crate::{
    DevicePreparationContext, DeviceResourceView, DeviceResources, INSTRUMENT_RACK_TYPE_ID,
    InstrumentHistoryRetargetPreparation, MAX_MAIN_PASS_DELAY_FRAMES, MutableInstrumentRuntime,
    NativeInstrumentClock, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentOccurrenceGain, NativeInstrumentRegistry, NativeInstrumentRuntimeError,
    NativeInstrumentRuntimeLimits, OffsetClock, PreparedAnalyticNote,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime, PreparedNativeInstrument,
    StereoFrame,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceContractError, DeviceTypeId,
    MAX_DEVICE_ASSET_REFERENCES, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::{
    any::Any,
    num::{NonZeroU8, NonZeroU64},
    sync::Arc,
};

/// Reserved native instrument type ID for a bounded Drum Rack.
pub const DRUM_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.drum");
/// Current v1 wire format for bounded Drum Rack state (`HXDR`).
pub const DRUM_RACK_STATE_VERSION: u32 = 1;
pub const DRUM_RACK_RUNTIME_VERSION: u32 = 1;
pub const DRUM_RACK_MAX_PADS: usize = 16;
pub const DRUM_RACK_MAX_CHOKE_GROUPS: u8 = 16;
pub const DRUM_RACK_MAX_PAD_NAME_BYTES: usize = 48;
pub const DRUM_RACK_MAX_PAD_GAIN: f32 = 4.0;
pub const DRUM_RACK_MAX_COMPENSATION_DELAY_FRAMES: u32 = MAX_MAIN_PASS_DELAY_FRAMES;
pub const DRUM_RACK_MAX_EVENTS_PER_BLOCK: usize = 259;
pub const DRUM_RACK_MAX_ACTIVE_OCCURRENCES: usize = 64;
const DRUM_RACK_CHUNK_FRAMES: usize = 256;
const DRUM_RACK_STATE_MAGIC: &[u8; 4] = b"HXDR";

pub const PARAM_DRUM_RACK_GAIN: ParameterId = ParameterId::from_static(1);

const DESCRIPTORS: [ParameterDescriptor; 1] = [ParameterDescriptor {
    id: PARAM_DRUM_RACK_GAIN,
    label: "drum rack gain",
    kind: ParameterKind::Float {
        min: 0.0,
        max: DRUM_RACK_MAX_PAD_GAIN,
        unit: None,
    },
    default: ParameterValue::Float(1.0),
    presentation: ParameterPresentation::Linear,
}];

/// Authored specification for one pad in a bounded Drum Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct DrumRackPadSpec {
    pub pad_id: NonZeroU64,
    pub device_id: NonZeroU64,
    pub name: String,
    pub receive_note: u8,
    pub play_note: u8,
    pub choke_group: Option<NonZeroU8>,
    pub gain: f32,
    pub muted: bool,
    /// `None` represents an empty or cleared pad slot.
    pub state: Option<NativeDeviceState>,
}

/// Authored specification for a bounded Drum Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct DrumRackSpec {
    pub rack_id: NonZeroU64,
    pub gain: f32,
    pub pads: Vec<DrumRackPadSpec>,
}

impl DrumRackSpec {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if !self.gain.is_finite() || !(0.0..=DRUM_RACK_MAX_PAD_GAIN).contains(&self.gain) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if self.pads.len() > DRUM_RACK_MAX_PADS {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        let mut seen_pads = [0_u64; DRUM_RACK_MAX_PADS];
        let mut seen_devices = [0_u64; DRUM_RACK_MAX_PADS];
        let mut seen_notes = [false; 128];
        let mut total_asset_refs = 0_usize;

        for (index, pad) in self.pads.iter().enumerate() {
            if pad.name.trim().is_empty() || pad.name.len() > DRUM_RACK_MAX_PAD_NAME_BYTES {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            if pad.receive_note > 127 || pad.play_note > 127 {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            if let Some(group) = pad.choke_group
                && group.get() > DRUM_RACK_MAX_CHOKE_GROUPS
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            if !pad.gain.is_finite() || !(0.0..=DRUM_RACK_MAX_PAD_GAIN).contains(&pad.gain) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            if seen_pads[..index].contains(&pad.pad_id.get())
                || seen_devices[..index].contains(&pad.device_id.get())
            {
                return Err(DeviceContractError::InvalidId);
            }
            if seen_notes[usize::from(pad.receive_note)] {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            seen_pads[index] = pad.pad_id.get();
            seen_devices[index] = pad.device_id.get();
            seen_notes[usize::from(pad.receive_note)] = true;

            if let Some(child) = &pad.state {
                if child.type_id() == DRUM_RACK_TYPE_ID
                    || child.type_id() == INSTRUMENT_RACK_TYPE_ID
                {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                total_asset_refs = total_asset_refs
                    .checked_add(child.asset_references().len())
                    .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                if total_asset_refs > MAX_DEVICE_ASSET_REFERENCES {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
            }
        }
        Ok(())
    }

    pub fn pad_by_id(&self, pad_id: NonZeroU64) -> Option<&DrumRackPadSpec> {
        self.pads.iter().find(|pad| pad.pad_id == pad_id)
    }

    pub fn rename_pad(
        &mut self,
        pad_id: NonZeroU64,
        name: impl Into<String>,
    ) -> Result<(), DeviceContractError> {
        let mut candidate = self.clone();
        let Some(pad) = candidate.pads.iter_mut().find(|p| p.pad_id == pad_id) else {
            return Err(DeviceContractError::InvalidId);
        };
        pad.name = name.into();
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn remap_pad(
        &mut self,
        pad_id: NonZeroU64,
        receive_note: u8,
        play_note: u8,
    ) -> Result<(), DeviceContractError> {
        let mut candidate = self.clone();
        let Some(pad) = candidate.pads.iter_mut().find(|p| p.pad_id == pad_id) else {
            return Err(DeviceContractError::InvalidId);
        };
        pad.receive_note = receive_note;
        pad.play_note = play_note;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn set_pad_choke_group(
        &mut self,
        pad_id: NonZeroU64,
        choke_group: Option<NonZeroU8>,
    ) -> Result<(), DeviceContractError> {
        let mut candidate = self.clone();
        let Some(pad) = candidate.pads.iter_mut().find(|p| p.pad_id == pad_id) else {
            return Err(DeviceContractError::InvalidId);
        };
        pad.choke_group = choke_group;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn set_pad_state(
        &mut self,
        pad_id: NonZeroU64,
        state: Option<NativeDeviceState>,
    ) -> Result<(), DeviceContractError> {
        let mut candidate = self.clone();
        let Some(pad) = candidate.pads.iter_mut().find(|p| p.pad_id == pad_id) else {
            return Err(DeviceContractError::InvalidId);
        };
        pad.state = state;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn remove_pad(&mut self, pad_id: NonZeroU64) -> Result<(), DeviceContractError> {
        let Some(index) = self.pads.iter().position(|p| p.pad_id == pad_id) else {
            return Err(DeviceContractError::InvalidId);
        };
        self.pads.remove(index);
        self.validate()
    }

    /// Compute the deterministic outer `DeviceAssetReference` list and per-pad
    /// `(outer_slot, child_slot)` remap table in canonical pad order.
    fn compute_asset_bindings(&self) -> Result<DrumRackAssetBindings, DeviceContractError> {
        let mut outer_refs = Vec::new();
        let mut per_pad_maps = Vec::with_capacity(self.pads.len());
        let mut next_outer_slot = 1_u32;

        for pad in &self.pads {
            let mut pad_map = Vec::new();
            if let Some(child) = &pad.state {
                pad_map.reserve_exact(child.asset_references().len());
                for reference in child.asset_references() {
                    let outer_slot = DeviceAssetSlot::new(next_outer_slot)?;
                    next_outer_slot = next_outer_slot
                        .checked_add(1)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    outer_refs.push(DeviceAssetReference {
                        slot: outer_slot,
                        asset: reference.asset,
                    });
                    pad_map.push((outer_slot, reference.slot));
                }
            }
            per_pad_maps.push(pad_map);
        }
        if outer_refs.len() > MAX_DEVICE_ASSET_REFERENCES {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(DrumRackAssetBindings {
            outer_refs,
            per_pad_maps,
        })
    }

    /// Encode this validated Drum Rack into a current v1 `NativeDeviceState`,
    /// aggregating child asset references onto the outer state envelope.
    pub fn encode_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let bindings = self.compute_asset_bindings()?;
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(DRUM_RACK_STATE_MAGIC);
        bytes.extend_from_slice(&self.rack_id.get().to_le_bytes());
        bytes.extend_from_slice(&self.gain.to_le_bytes());
        let pad_count = u8::try_from(self.pads.len())
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        bytes.push(pad_count);

        for pad in &self.pads {
            bytes.extend_from_slice(&pad.pad_id.get().to_le_bytes());
            bytes.extend_from_slice(&pad.device_id.get().to_le_bytes());
            let name_bytes = pad.name.as_bytes();
            let name_len = u16::try_from(name_bytes.len())
                .map_err(|_| DeviceContractError::InvalidParameterValue)?;
            bytes.extend_from_slice(&name_len.to_le_bytes());
            bytes.extend_from_slice(name_bytes);
            bytes.push(pad.receive_note);
            bytes.push(pad.play_note);
            bytes.push(pad.choke_group.map_or(0, NonZeroU8::get));
            bytes.extend_from_slice(&pad.gain.to_le_bytes());
            bytes.push(u8::from(pad.muted));

            match &pad.state {
                None => bytes.push(0),
                Some(child) => {
                    bytes.push(1);
                    let child_type = child.type_id();
                    let type_str = child_type.as_str()?;
                    let type_bytes = type_str.as_bytes();
                    let type_len = u16::try_from(type_bytes.len())
                        .map_err(|_| DeviceContractError::StateTooLarge)?;
                    bytes.extend_from_slice(&type_len.to_le_bytes());
                    bytes.extend_from_slice(type_bytes);
                    bytes.extend_from_slice(&child.state_version().to_le_bytes());
                    bytes.extend_from_slice(&child.input_gain_millidb().to_le_bytes());
                    bytes.extend_from_slice(&child.output_gain_millidb().to_le_bytes());
                    let payload = child.payload();
                    let payload_len = u32::try_from(payload.len())
                        .map_err(|_| DeviceContractError::StateTooLarge)?;
                    bytes.extend_from_slice(&payload_len.to_le_bytes());
                    bytes.extend_from_slice(payload);
                    let refs = child.asset_references();
                    let ref_count = u16::try_from(refs.len())
                        .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
                    bytes.extend_from_slice(&ref_count.to_le_bytes());
                    for reference in refs {
                        bytes.extend_from_slice(&reference.slot.value().to_le_bytes());
                        bytes.extend_from_slice(&reference.asset.value().to_le_bytes());
                    }
                }
            }
        }

        if bytes.len() > MAX_NATIVE_DEVICE_STATE_BYTES {
            return Err(DeviceContractError::StateTooLarge);
        }
        let state = NativeDeviceState::new(DRUM_RACK_TYPE_ID, DRUM_RACK_STATE_VERSION, bytes)?;
        state.with_asset_references(bindings.outer_refs)
    }

    /// Decode and strictly validate a Drum Rack `NativeDeviceState`.
    pub fn decode_and_migrate_state(
        state: &NativeDeviceState,
    ) -> Result<Self, DeviceContractError> {
        if state.type_id() != DRUM_RACK_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != DRUM_RACK_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let bytes = state.payload();
        let mut cursor = 0_usize;
        let magic = read_slice(bytes, &mut cursor, 4)?;
        if magic != DRUM_RACK_STATE_MAGIC {
            return Err(DeviceContractError::MalformedState);
        }
        let rack_id =
            NonZeroU64::new(read_u64(bytes, &mut cursor)?).ok_or(DeviceContractError::InvalidId)?;
        let gain = f32::from_le_bytes(read_array::<4>(bytes, &mut cursor)?);
        let pad_count = usize::from(read_u8(bytes, &mut cursor)?);
        if pad_count > DRUM_RACK_MAX_PADS {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }

        let mut pads = Vec::with_capacity(pad_count);
        for _ in 0..pad_count {
            let pad_id = NonZeroU64::new(read_u64(bytes, &mut cursor)?)
                .ok_or(DeviceContractError::InvalidId)?;
            let device_id = NonZeroU64::new(read_u64(bytes, &mut cursor)?)
                .ok_or(DeviceContractError::InvalidId)?;
            let name_len = usize::from(read_u16(bytes, &mut cursor)?);
            if name_len == 0 || name_len > DRUM_RACK_MAX_PAD_NAME_BYTES {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            let name_bytes = read_slice(bytes, &mut cursor, name_len)?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| DeviceContractError::MalformedState)?
                .to_owned();
            let receive_note = read_u8(bytes, &mut cursor)?;
            let play_note = read_u8(bytes, &mut cursor)?;
            let raw_choke = read_u8(bytes, &mut cursor)?;
            let choke_group = NonZeroU8::new(raw_choke);
            let pad_gain = f32::from_le_bytes(read_array::<4>(bytes, &mut cursor)?);
            let muted = match read_u8(bytes, &mut cursor)? {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            let child_state = match read_u8(bytes, &mut cursor)? {
                0 => None,
                1 => {
                    let type_len = usize::from(read_u16(bytes, &mut cursor)?);
                    let type_bytes = read_slice(bytes, &mut cursor, type_len)?;
                    let type_str = std::str::from_utf8(type_bytes)
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    let type_id = DeviceTypeId::new(type_str)?;
                    let version = read_u32(bytes, &mut cursor)?;
                    let input_gain_millidb =
                        i32::from_le_bytes(read_array::<4>(bytes, &mut cursor)?);
                    let output_gain_millidb =
                        i32::from_le_bytes(read_array::<4>(bytes, &mut cursor)?);
                    let payload_len = usize::try_from(read_u32(bytes, &mut cursor)?)
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    let payload = read_slice(bytes, &mut cursor, payload_len)?;
                    let ref_count = usize::from(read_u16(bytes, &mut cursor)?);
                    if ref_count > MAX_DEVICE_ASSET_REFERENCES {
                        return Err(DeviceContractError::RuntimeBudgetExceeded);
                    }
                    let mut refs = Vec::with_capacity(ref_count);
                    for _ in 0..ref_count {
                        let slot = DeviceAssetSlot::new(read_u32(bytes, &mut cursor)?)?;
                        let asset = DeviceAssetKey::new(read_u64(bytes, &mut cursor)?)?;
                        refs.push(DeviceAssetReference { slot, asset });
                    }
                    let child = NativeDeviceState::new(type_id, version, payload.to_vec())?
                        .with_input_gain_millidb(input_gain_millidb)?
                        .with_output_gain_millidb(output_gain_millidb)?
                        .with_asset_references(refs)?;
                    Some(child)
                }
                _ => return Err(DeviceContractError::MalformedState),
            };

            pads.push(DrumRackPadSpec {
                pad_id,
                device_id,
                name,
                receive_note,
                play_note,
                choke_group,
                gain: pad_gain,
                muted,
                state: child_state,
            });
        }

        if cursor != bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }

        let spec = Self {
            rack_id,
            gain,
            pads,
        };
        spec.validate()?;
        let bindings = spec.compute_asset_bindings()?;
        if state.asset_references() != bindings.outer_refs.as_slice() {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(spec)
    }
}

struct DrumRackAssetBindings {
    outer_refs: Vec<DeviceAssetReference>,
    per_pad_maps: Vec<Vec<(DeviceAssetSlot, DeviceAssetSlot)>>,
}

/// Zero-allocation stack view presenting one pad's expected child asset slots
/// backed by the host-retained outer `DeviceResources`.
struct PadDeviceResources<'a> {
    parent: &'a dyn DeviceResources,
    mappings: &'a [(DeviceAssetSlot, DeviceAssetSlot)],
}

impl DeviceResources for PadDeviceResources<'_> {
    fn len(&self) -> usize {
        self.mappings.len()
    }

    fn get(&self, index: usize) -> Option<DeviceResourceView<'_>> {
        let &(outer_slot, child_slot) = self.mappings.get(index)?;
        let mut view = self.parent.slot(outer_slot)?;
        view.slot = child_slot;
        Some(view)
    }
}

fn read_slice<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
) -> Result<&'a [u8], DeviceContractError> {
    let end = cursor
        .checked_add(len)
        .ok_or(DeviceContractError::MalformedState)?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or(DeviceContractError::MalformedState)?;
    *cursor = end;
    Ok(slice)
}

fn read_array<const N: usize>(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<[u8; N], DeviceContractError> {
    let slice = read_slice(bytes, cursor, N)?;
    let mut out = [0_u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, DeviceContractError> {
    Ok(read_array::<1>(bytes, cursor)?[0])
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, DeviceContractError> {
    Ok(u16::from_le_bytes(read_array::<2>(bytes, cursor)?))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, DeviceContractError> {
    Ok(u32::from_le_bytes(read_array::<4>(bytes, cursor)?))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, DeviceContractError> {
    Ok(u64::from_le_bytes(read_array::<8>(bytes, cursor)?))
}

/// Native instrument definition for a bounded Drum Rack backed by an existing
/// child `NativeInstrumentRegistry`.
pub struct DrumRackDefinition<'a> {
    children: &'a NativeInstrumentRegistry<'a>,
}

impl<'a> DrumRackDefinition<'a> {
    pub fn new(children: &'a NativeInstrumentRegistry<'a>) -> Result<Self, DeviceContractError> {
        if children.definition(DRUM_RACK_TYPE_ID).is_some() {
            return Err(DeviceContractError::DuplicateDeviceType);
        }
        Ok(Self { children })
    }

    fn prepare_rack(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: Option<NativeInstrumentRuntimeLimits>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        context.validate_for(state)?;
        let spec = DrumRackSpec::decode_and_migrate_state(state)?;
        let bindings = spec.compute_asset_bindings()?;

        let mut prepared_pads: Vec<PreparedPadFactory> = Vec::new();
        let mut pad_latencies: Vec<u32> = Vec::new();

        for (pad, slot_map) in spec.pads.iter().zip(bindings.per_pad_maps) {
            if pad.muted {
                continue;
            }
            let Some(child_state) = &pad.state else {
                continue;
            };
            let pad_resources = PadDeviceResources {
                parent: context.resources,
                mappings: &slot_map,
            };
            let pad_context = DevicePreparationContext {
                sample_rate: context.sample_rate,
                tuning: context.tuning,
                resources: &pad_resources,
            };
            let prepared_inst = match limits {
                Some(child_limits) => self.children.prepare_instrument_with_runtime_limits(
                    child_state,
                    &pad_context,
                    child_limits,
                )?,
                None => self
                    .children
                    .prepare_instrument(child_state, &pad_context)?,
            };
            let latency = prepared_inst.algorithmic_latency_frames().unwrap_or(0);
            let Some(factory) = prepared_inst.prepare_mutable_runtime()? else {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            };
            let child_latency = factory.algorithmic_latency_frames().unwrap_or(latency);
            if child_latency > DRUM_RACK_MAX_COMPENSATION_DELAY_FRAMES {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            pad_latencies.push(child_latency);
            prepared_pads.push(PreparedPadFactory {
                pad_id: pad.pad_id,
                device_id: pad.device_id,
                receive_note: pad.receive_note,
                play_note: pad.play_note,
                choke_group: pad.choke_group,
                gain: pad.gain,
                compensation_delay_frames: 0,
                asset_slot_map: slot_map,
                factory,
            });
        }

        let rack_latency_frames = pad_latencies.iter().copied().max().unwrap_or(0);
        if rack_latency_frames > DRUM_RACK_MAX_COMPENSATION_DELAY_FRAMES {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }

        for (pad, latency) in prepared_pads.iter_mut().zip(&pad_latencies) {
            let comp = rack_latency_frames.saturating_sub(*latency);
            if comp > DRUM_RACK_MAX_COMPENSATION_DELAY_FRAMES {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            pad.compensation_delay_frames = comp;
        }

        let mut max_events = DRUM_RACK_MAX_EVENTS_PER_BLOCK;
        let mut max_active = DRUM_RACK_MAX_ACTIVE_OCCURRENCES;
        let mut max_tail = 0_u64;
        let mut supports_gain = true;
        let mut supports_active_ids = true;
        let mut steals_releasing = true;
        let any_active = !prepared_pads.is_empty();

        for pad in &prepared_pads {
            max_events = max_events.min(pad.factory.max_events_per_block());
            max_active = max_active.min(pad.factory.max_active_occurrences());
            let pad_tail = pad
                .factory
                .max_release_tail_frames()
                .checked_add(u64::from(pad.compensation_delay_frames))
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            max_tail = max_tail.max(pad_tail);
            supports_gain &= pad.factory.supports_occurrence_gain();
            supports_active_ids &= pad.factory.supports_active_occurrence_ids();
            steals_releasing &= pad.factory.steals_releasing_voices();
        }

        if !any_active {
            supports_gain = true;
            supports_active_ids = true;
            steals_releasing = false;
        }
        if max_events == 0 || max_active == 0 {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }

        // Exact control-allocated runtime storage calculation matching fixed_storage_bytes().
        let pads_vec_bytes = prepared_pads
            .len()
            .checked_mul(std::mem::size_of::<PadRuntime>())
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        let chunk_events_bytes = max_events
            .checked_mul(std::mem::size_of::<NativeInstrumentEvent>())
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        let mut twin_bytes = std::mem::size_of::<DrumRackRuntime>()
            .checked_add(pads_vec_bytes)
            .and_then(|sum| sum.checked_add(chunk_events_bytes))
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;

        for pad in &prepared_pads {
            let delay_bytes = usize::try_from(pad.compensation_delay_frames)
                .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?
                .checked_mul(std::mem::size_of::<StereoFrame>())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            let scratch_events_bytes = max_events
                .checked_mul(std::mem::size_of::<NativeInstrumentEvent>())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            let slot_map_bytes = pad
                .asset_slot_map
                .len()
                .checked_mul(std::mem::size_of::<(DeviceAssetSlot, DeviceAssetSlot)>())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            twin_bytes = twin_bytes
                .checked_add(pad.factory.declared_twin_bytes())
                .and_then(|sum| sum.checked_add(delay_bytes))
                .and_then(|sum| sum.checked_add(scratch_events_bytes))
                .and_then(|sum| sum.checked_add(slot_map_bytes))
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        }

        if let Some(host_limits) = limits {
            let pair_bytes = twin_bytes
                .checked_mul(2)
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            if pair_bytes > host_limits.max_pair_bytes
                || max_tail > host_limits.max_tail_frames
                || max_events > host_limits.max_events_per_block
                || max_active > host_limits.max_active_occurrences
            {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
        }

        Ok(Box::new(PreparedDrumRack {
            rack_id: spec.rack_id,
            gain: spec.gain,
            sample_rate: context.sample_rate,
            rack_latency_frames,
            max_events_per_block: max_events,
            max_active_occurrences: max_active,
            max_release_tail_frames: max_tail,
            declared_twin_bytes: twin_bytes,
            supports_occurrence_gain: supports_gain,
            supports_active_occurrence_ids: supports_active_ids,
            steals_releasing_voices: steals_releasing,
            pads: Arc::new(prepared_pads),
        }))
    }
}

impl NativeDeviceMetadata for DrumRackDefinition<'_> {
    fn type_id(&self) -> DeviceTypeId {
        DRUM_RACK_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("drum rack")
    }

    fn state_version(&self) -> u32 {
        DRUM_RACK_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        let Some(rack_id) = NonZeroU64::new(1) else {
            return Err(DeviceContractError::InvalidId);
        };
        DrumRackSpec {
            rack_id,
            gain: 1.0,
            pads: Vec::new(),
        }
        .encode_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = DrumRackSpec::decode_and_migrate_state(state)?;
        for pad in &mut spec.pads {
            if let Some(child) = &pad.state {
                pad.state = Some(self.children.migrate_state(child)?);
            }
        }
        spec.encode_state()
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let spec = DrumRackSpec::decode_and_migrate_state(state)?;
        match parameter {
            PARAM_DRUM_RACK_GAIN => Ok(ParameterValue::Float(spec.gain)),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = DrumRackSpec::decode_and_migrate_state(state)?;
        match (parameter, value) {
            (PARAM_DRUM_RACK_GAIN, ParameterValue::Float(v))
                if v.is_finite() && (0.0..=DRUM_RACK_MAX_PAD_GAIN).contains(&v) =>
            {
                spec.gain = v;
            }
            (PARAM_DRUM_RACK_GAIN, _) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        spec.encode_state()
    }
}

impl NativeInstrumentDefinition for DrumRackDefinition<'_> {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<PreparedDrumRackRuntime>())
    }

    fn supports_asset_resources(&self) -> bool {
        true
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_rack(state, context, None)
    }

    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_rack(state, context, Some(limits))
    }
}

struct PreparedPadFactory {
    pad_id: NonZeroU64,
    device_id: NonZeroU64,
    receive_note: u8,
    play_note: u8,
    choke_group: Option<NonZeroU8>,
    gain: f32,
    compensation_delay_frames: u32,
    asset_slot_map: Vec<(DeviceAssetSlot, DeviceAssetSlot)>,
    factory: Box<dyn PreparedMutableInstrumentRuntime>,
}

struct PreparedDrumRack {
    rack_id: NonZeroU64,
    gain: f32,
    sample_rate: u32,
    rack_latency_frames: u32,
    max_events_per_block: usize,
    max_active_occurrences: usize,
    max_release_tail_frames: u64,
    declared_twin_bytes: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    steals_releasing_voices: bool,
    pads: Arc<Vec<PreparedPadFactory>>,
}

impl PreparedNativeInstrument for PreparedDrumRack {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.rack_latency_frames)
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
        Ok(Some(Box::new(PreparedDrumRackRuntime {
            rack_id: self.rack_id,
            gain: self.gain,
            sample_rate: self.sample_rate,
            rack_latency_frames: self.rack_latency_frames,
            max_events_per_block: self.max_events_per_block,
            max_active_occurrences: self.max_active_occurrences,
            max_release_tail_frames: self.max_release_tail_frames,
            declared_twin_bytes: self.declared_twin_bytes,
            supports_occurrence_gain: self.supports_occurrence_gain,
            supports_active_occurrence_ids: self.supports_active_occurrence_ids,
            steals_releasing_voices: self.steals_releasing_voices,
            pads: Arc::clone(&self.pads),
        })))
    }
}

struct PreparedDrumRackRuntime {
    rack_id: NonZeroU64,
    gain: f32,
    sample_rate: u32,
    rack_latency_frames: u32,
    max_events_per_block: usize,
    max_active_occurrences: usize,
    max_release_tail_frames: u64,
    declared_twin_bytes: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    steals_releasing_voices: bool,
    pads: Arc<Vec<PreparedPadFactory>>,
}

struct DrumRackRetargetProof {
    rack_id: NonZeroU64,
    sample_rate: u32,
}

impl PreparedInstrumentHistoryRetarget for DrumRackRetargetProof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedMutableInstrumentRuntime for PreparedDrumRackRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.supports_active_occurrence_ids
    }

    fn supports_occurrence_gain(&self) -> bool {
        self.supports_occurrence_gain
    }

    fn steals_releasing_voices(&self) -> bool {
        self.steals_releasing_voices
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.rack_latency_frames)
    }

    fn runtime_version(&self) -> u32 {
        DRUM_RACK_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        self.declared_twin_bytes
    }

    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn max_events_per_block(&self) -> usize {
        self.max_events_per_block
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    fn max_release_tail_frames(&self) -> u64 {
        self.max_release_tail_frames
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let mut pads = Vec::with_capacity(self.pads.len());
        let mut note_to_pad = [None; 128];

        for (index, prepared) in self.pads.iter().enumerate() {
            let delay = if prepared.compensation_delay_frames > 0 {
                Some(DrumCompensationDelay::new_on_control(
                    prepared.compensation_delay_frames as usize,
                ))
            } else {
                None
            };
            let mut asset_slot_map = Vec::with_capacity(prepared.asset_slot_map.len());
            asset_slot_map.extend_from_slice(&prepared.asset_slot_map);
            let event_scratch = vec![
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                };
                self.max_events_per_block
            ];
            note_to_pad[usize::from(prepared.receive_note)] = Some(index as u8);
            pads.push(PadRuntime {
                pad_id: prepared.pad_id,
                device_id: prepared.device_id,
                receive_note: prepared.receive_note,
                play_note: prepared.play_note,
                choke_group: prepared.choke_group,
                gain: prepared.gain,
                compensation_delay_frames: prepared.compensation_delay_frames,
                asset_slot_map,
                runtime: prepared.factory.create_runtime(),
                delay,
                event_scratch,
                event_len: 0,
            });
        }

        let chunk_events = vec![
            NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            };
            self.max_events_per_block
        ];

        Box::new(DrumRackRuntime {
            rack_id: self.rack_id,
            gain: self.gain,
            sample_rate: self.sample_rate,
            max_events_per_block: self.max_events_per_block,
            supports_occurrence_gain: self.supports_occurrence_gain,
            supports_active_occurrence_ids: self.supports_active_occurrence_ids,
            pads,
            note_to_pad,
            routed: [None; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
            chunk_events,
            pad_scratch: [[0.0; 2]; DRUM_RACK_CHUNK_FRAMES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            DrumRackRetargetProof {
                rack_id: self.rack_id,
                sample_rate: self.sample_rate,
            },
        )))
    }
}

struct DrumCompensationDelay {
    frames: Vec<StereoFrame>,
    next: usize,
}

impl DrumCompensationDelay {
    fn new_on_control(len: usize) -> Self {
        Self {
            frames: vec![[0.0; 2]; len],
            next: 0,
        }
    }

    fn process(&mut self, samples: &mut [StereoFrame]) {
        let len = self.frames.len();
        if len == 0 {
            return;
        }
        for sample in samples {
            let out = self.frames[self.next];
            self.frames[self.next] = *sample;
            self.next = (self.next + 1) % len;
            *sample = out;
        }
    }

    fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.next = 0;
    }

    fn is_quiet(&self) -> bool {
        self.frames.iter().all(|f| f[0] == 0.0 && f[1] == 0.0)
    }

    fn copy_from(&mut self, other: &Self) {
        if self.frames.len() == other.frames.len() {
            self.frames.copy_from_slice(&other.frames);
            self.next = other.next;
        }
    }

    fn storage_bytes(&self) -> usize {
        self.frames
            .capacity()
            .saturating_mul(std::mem::size_of::<StereoFrame>())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RoutedPadOccurrence {
    occurrence_id: u64,
    pad_index: u8,
    note_off_sent: bool,
}

struct PadRuntime {
    pad_id: NonZeroU64,
    device_id: NonZeroU64,
    receive_note: u8,
    play_note: u8,
    choke_group: Option<NonZeroU8>,
    gain: f32,
    compensation_delay_frames: u32,
    asset_slot_map: Vec<(DeviceAssetSlot, DeviceAssetSlot)>,
    runtime: Box<dyn MutableInstrumentRuntime>,
    delay: Option<DrumCompensationDelay>,
    event_scratch: Vec<NativeInstrumentEvent>,
    event_len: usize,
}

struct DrumRackRuntime {
    rack_id: NonZeroU64,
    gain: f32,
    sample_rate: u32,
    max_events_per_block: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    pads: Vec<PadRuntime>,
    note_to_pad: [Option<u8>; 128],
    routed: [Option<RoutedPadOccurrence>; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
    chunk_events: Vec<NativeInstrumentEvent>,
    pad_scratch: [StereoFrame; DRUM_RACK_CHUNK_FRAMES],
}

impl DrumRackRuntime {
    fn find_routed(
        routed: &[Option<RoutedPadOccurrence>; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
        occurrence_id: u64,
    ) -> Option<usize> {
        routed
            .iter()
            .position(|slot| slot.is_some_and(|r| r.occurrence_id == occurrence_id))
    }

    fn find_free_routed(
        routed: &[Option<RoutedPadOccurrence>; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
    ) -> Option<usize> {
        routed.iter().position(Option::is_none)
    }

    /// Extract all events destined for `target_pad` while simulating shared
    /// choke-group state across all pads in exact chronological order.
    fn simulate_events_for_pad(
        &self,
        target_pad: usize,
        events: &[NativeInstrumentEvent],
        out: &mut [NativeInstrumentEvent; DRUM_RACK_MAX_EVENTS_PER_BLOCK],
    ) -> Result<
        (
            usize,
            [Option<RoutedPadOccurrence>; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
        ),
        NativeInstrumentRuntimeError,
    > {
        let mut sim_routed = self.routed;
        let mut len = 0_usize;
        let cap = self.max_events_per_block.min(out.len());

        for &event in events {
            match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => {
                    let Some(pad_idx_u8) =
                        self.note_to_pad.get(usize::from(pitch)).copied().flatten()
                    else {
                        continue;
                    };
                    let pad_idx = usize::from(pad_idx_u8);
                    let Some(matched_pad) = self.pads.get(pad_idx) else {
                        continue;
                    };
                    if let Some(group) = matched_pad.choke_group {
                        for slot in &mut sim_routed {
                            if let Some(entry) = slot.as_mut()
                                && usize::from(entry.pad_index) != pad_idx
                                && !entry.note_off_sent
                                && self
                                    .pads
                                    .get(usize::from(entry.pad_index))
                                    .is_some_and(|other| other.choke_group == Some(group))
                            {
                                entry.note_off_sent = true;
                                if usize::from(entry.pad_index) == target_pad {
                                    if len >= cap {
                                        return Err(
                                            NativeInstrumentRuntimeError::EventCapacityExceeded,
                                        );
                                    }
                                    out[len] = NativeInstrumentEvent::NoteOff {
                                        frame_offset,
                                        occurrence_id: entry.occurrence_id,
                                    };
                                    len += 1;
                                }
                            }
                        }
                    }

                    if let Some(existing) = Self::find_routed(&sim_routed, occurrence_id) {
                        sim_routed[existing] = Some(RoutedPadOccurrence {
                            occurrence_id,
                            pad_index: pad_idx_u8,
                            note_off_sent: false,
                        });
                    } else if let Some(free) = Self::find_free_routed(&sim_routed) {
                        sim_routed[free] = Some(RoutedPadOccurrence {
                            occurrence_id,
                            pad_index: pad_idx_u8,
                            note_off_sent: false,
                        });
                    } else {
                        return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
                    }

                    if pad_idx == target_pad {
                        if len >= cap {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        }
                        out[len] = NativeInstrumentEvent::NoteOn {
                            frame_offset,
                            occurrence_id,
                            pitch: matched_pad.play_note,
                            velocity,
                        };
                        len += 1;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_routed(&sim_routed, occurrence_id)
                        && let Some(entry) = sim_routed[slot].as_mut()
                        && !entry.note_off_sent
                    {
                        entry.note_off_sent = true;
                        if usize::from(entry.pad_index) == target_pad {
                            if len >= cap {
                                return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                            }
                            out[len] = event;
                            len += 1;
                        }
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_routed(&sim_routed, occurrence_id.get())
                        && let Some(entry) = sim_routed[slot]
                        && usize::from(entry.pad_index) == target_pad
                    {
                        if len >= cap {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        }
                        out[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    if len >= cap {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    }
                    out[len] = event;
                    len += 1;
                }
            }
        }
        Ok((len, sim_routed))
    }

    fn filter_gains_for_pad(
        routed: &[Option<RoutedPadOccurrence>; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
        pad_index: usize,
        caller_gains: &[NativeInstrumentOccurrenceGain],
        out: &mut [NativeInstrumentOccurrenceGain; DRUM_RACK_MAX_ACTIVE_OCCURRENCES],
    ) -> usize {
        let mut len = 0_usize;
        for &gain in caller_gains {
            if let Some(slot) = Self::find_routed(routed, gain.occurrence_id())
                && let Some(entry) = routed[slot]
                && usize::from(entry.pad_index) == pad_index
                && len < out.len()
            {
                out[len] = gain;
                len += 1;
            }
        }
        len
    }

    fn prune_finished_routed(&mut self) {
        let mut active_buf = [0_u64; DRUM_RACK_MAX_ACTIVE_OCCURRENCES];
        for (pad_idx, pad) in self.pads.iter().enumerate() {
            if pad.runtime.is_quiescent() {
                for slot in &mut self.routed {
                    if let Some(entry) = *slot
                        && usize::from(entry.pad_index) == pad_idx
                    {
                        *slot = None;
                    }
                }
            } else if pad.runtime.supports_active_occurrence_ids()
                && let Ok(n) = pad.runtime.active_occurrence_ids(&mut active_buf)
            {
                for slot in &mut self.routed {
                    if let Some(entry) = *slot
                        && usize::from(entry.pad_index) == pad_idx
                        && entry.note_off_sent
                        && !active_buf[..n].contains(&entry.occurrence_id)
                    {
                        *slot = None;
                    }
                }
            }
        }
    }
}

fn shift_event_offset(event: NativeInstrumentEvent, local_offset: usize) -> NativeInstrumentEvent {
    match event {
        NativeInstrumentEvent::Expression { expression, .. } => NativeInstrumentEvent::Expression {
            frame_offset: local_offset,
            expression,
        },
        NativeInstrumentEvent::ControlChange { cc, value, .. } => {
            NativeInstrumentEvent::ControlChange {
                frame_offset: local_offset,
                cc,
                value,
            }
        }
        NativeInstrumentEvent::PitchBend { value, .. } => NativeInstrumentEvent::PitchBend {
            frame_offset: local_offset,
            value,
        },
        NativeInstrumentEvent::NotePressure {
            occurrence_id,
            value,
            ..
        } => NativeInstrumentEvent::NotePressure {
            frame_offset: local_offset,
            occurrence_id,
            value,
        },
        NativeInstrumentEvent::NotePitchBend {
            occurrence_id,
            cents,
            ..
        } => NativeInstrumentEvent::NotePitchBend {
            frame_offset: local_offset,
            occurrence_id,
            cents,
        },
        NativeInstrumentEvent::NoteTimbre {
            occurrence_id,
            value,
            ..
        } => NativeInstrumentEvent::NoteTimbre {
            frame_offset: local_offset,
            occurrence_id,
            value,
        },
        NativeInstrumentEvent::NoteOn {
            occurrence_id,
            pitch,
            velocity,
            ..
        } => NativeInstrumentEvent::NoteOn {
            frame_offset: local_offset,
            occurrence_id,
            pitch,
            velocity,
        },
        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => NativeInstrumentEvent::NoteOff {
            frame_offset: local_offset,
            occurrence_id,
        },
    }
}

impl MutableInstrumentRuntime for DrumRackRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.supports_active_occurrence_ids
    }

    fn supports_occurrence_gain(&self) -> bool {
        self.supports_occurrence_gain
    }

    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        if !self.supports_active_occurrence_ids {
            return Err(NativeInstrumentRuntimeError::UnsupportedActiveOccurrenceIds);
        }
        let mut merged = [0_u64; DRUM_RACK_MAX_ACTIVE_OCCURRENCES];
        let mut count = 0_usize;
        let mut pad_buf = [0_u64; DRUM_RACK_MAX_ACTIVE_OCCURRENCES];
        for pad in &self.pads {
            let n = pad.runtime.active_occurrence_ids(&mut pad_buf)?;
            for &id in &pad_buf[..n] {
                if !merged[..count].contains(&id) {
                    if count >= merged.len() {
                        return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
                    }
                    merged[count] = id;
                    count += 1;
                }
            }
        }
        merged[..count].sort_unstable();
        if count > output.len() {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        output[..count].copy_from_slice(&merged[..count]);
        Ok(count)
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > self.max_events_per_block {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut pad_events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; DRUM_RACK_MAX_EVENTS_PER_BLOCK];
        for (pad_idx, pad) in self.pads.iter().enumerate() {
            let (len, _) = self.simulate_events_for_pad(pad_idx, events, &mut pad_events)?;
            let pad_resources = PadDeviceResources {
                parent: resources,
                mappings: &pad.asset_slot_map,
            };
            pad.runtime
                .preflight_events(&pad_events[..len], frame_count, &pad_resources)?;
        }
        Ok(())
    }

    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        for pad in &self.pads {
            let pad_resources = PadDeviceResources {
                parent: resources,
                mappings: &pad.asset_slot_map,
            };
            pad.runtime.preflight_clock(clock, frames, &pad_resources)?;
        }
        Ok(())
    }

    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if gains.is_empty() {
            return Ok(());
        }
        if !self.supports_occurrence_gain {
            return Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain);
        }
        if gains.len() > DRUM_RACK_MAX_ACTIVE_OCCURRENCES {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        let mut pad_events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; DRUM_RACK_MAX_EVENTS_PER_BLOCK];
        let mut pad_gains = [gains[0]; DRUM_RACK_MAX_ACTIVE_OCCURRENCES];
        for (pad_idx, pad) in self.pads.iter().enumerate() {
            let (ev_len, sim_routed) =
                self.simulate_events_for_pad(pad_idx, events, &mut pad_events)?;
            let g_len = Self::filter_gains_for_pad(&sim_routed, pad_idx, gains, &mut pad_gains);
            if g_len > 0 {
                pad.runtime
                    .preflight_occurrence_gains(&pad_events[..ev_len], &pad_gains[..g_len])?;
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
        self.process_block_with_clock_and_occurrence_gains(events, frames, None, &[], resources);
    }

    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, clock, &[], resources);
    }

    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        let total_frames = frames.len();
        if total_frames == 0 || self.pads.is_empty() {
            return;
        }
        let Ok(dummy_gain) = NativeInstrumentOccurrenceGain::new(1, 1.0, 1.0, 1, 0) else {
            return;
        };
        let mut chunk_gains_storage = gains
            .first()
            .copied()
            .map(|first| [first; DRUM_RACK_MAX_ACTIVE_OCCURRENCES]);
        let mut pad_gains_buf = [dummy_gain; DRUM_RACK_MAX_ACTIVE_OCCURRENCES];

        let mut event_cursor = 0_usize;
        let mut chunk_start = 0_usize;
        while chunk_start < total_frames {
            let chunk_len = (total_frames - chunk_start).min(DRUM_RACK_CHUNK_FRAMES);
            let chunk_end = chunk_start + chunk_len;

            let mut chunk_event_len = 0_usize;
            while event_cursor < events.len() && events[event_cursor].frame_offset() < chunk_end {
                if chunk_event_len < self.chunk_events.len() {
                    let local_offset = events[event_cursor]
                        .frame_offset()
                        .saturating_sub(chunk_start);
                    self.chunk_events[chunk_event_len] =
                        shift_event_offset(events[event_cursor], local_offset);
                    chunk_event_len += 1;
                }
                event_cursor += 1;
            }

            // Route events into each pad's control-preallocated scratch buffer.
            for pad in &mut self.pads {
                pad.event_len = 0;
            }
            for idx in 0..chunk_event_len {
                let ev = self.chunk_events[idx];
                self.route_single_event(ev);
            }

            let active_gains: &[NativeInstrumentOccurrenceGain] =
                if let Some(storage) = chunk_gains_storage.as_mut() {
                    let gain_len = gains.len().min(storage.len());
                    for (dst, &src) in storage[..gain_len].iter_mut().zip(&gains[..gain_len]) {
                        *dst = src.advance_frames(chunk_start as u64).unwrap_or(src);
                    }
                    &storage[..gain_len]
                } else {
                    &[]
                };

            let offset_clock = clock.map(|base| OffsetClock {
                clock: base,
                offset: chunk_start,
            });
            let chunk_clock_ref: Option<&dyn NativeInstrumentClock> = offset_clock
                .as_ref()
                .map(|c| c as &dyn NativeInstrumentClock);
            let out_slice = &mut frames[chunk_start..chunk_end];
            let rack_gain = self.gain;

            for pad_idx in 0..self.pads.len() {
                let g_len = if self.supports_occurrence_gain && !active_gains.is_empty() {
                    Self::filter_gains_for_pad(
                        &self.routed,
                        pad_idx,
                        active_gains,
                        &mut pad_gains_buf,
                    )
                } else {
                    0
                };
                let scratch = &mut self.pad_scratch[..chunk_len];
                scratch.fill([0.0; 2]);
                let pad = &mut self.pads[pad_idx];
                let pad_resources = PadDeviceResources {
                    parent: resources,
                    mappings: &pad.asset_slot_map,
                };
                pad.runtime.process_block_with_clock_and_occurrence_gains(
                    &pad.event_scratch[..pad.event_len],
                    scratch,
                    chunk_clock_ref,
                    &pad_gains_buf[..g_len],
                    &pad_resources,
                );
                if let Some(delay) = &mut pad.delay {
                    delay.process(scratch);
                }
                let combined_gain = pad.gain * rack_gain;
                for (dst, &src) in out_slice.iter_mut().zip(scratch.iter()) {
                    dst[0] += src[0] * combined_gain;
                    dst[1] += src[1] * combined_gain;
                }
            }

            self.prune_finished_routed();
            chunk_start = chunk_end;
        }
    }

    fn reset(&mut self) {
        for pad in &mut self.pads {
            pad.runtime.reset();
            pad.event_len = 0;
            if let Some(delay) = &mut pad.delay {
                delay.reset();
            }
        }
        self.routed.fill(None);
    }

    fn is_quiescent(&self) -> bool {
        self.pads.iter().all(|pad| {
            pad.runtime.is_quiescent()
                && pad
                    .delay
                    .as_ref()
                    .is_none_or(DrumCompensationDelay::is_quiet)
        })
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        if self.rack_id != other.rack_id
            || self.gain.to_bits() != other.gain.to_bits()
            || self.sample_rate != other.sample_rate
            || self.max_events_per_block != other.max_events_per_block
            || self.pads.len() != other.pads.len()
        {
            return false;
        }
        self.pads.iter().zip(&other.pads).all(|(a, b)| {
            a.pad_id == b.pad_id
                && a.device_id == b.device_id
                && a.receive_note == b.receive_note
                && a.play_note == b.play_note
                && a.choke_group == b.choke_group
                && a.gain.to_bits() == b.gain.to_bits()
                && a.compensation_delay_frames == b.compensation_delay_frames
                && a.asset_slot_map == b.asset_slot_map
                && a.runtime.can_copy_history_from(b.runtime.as_ref())
        })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        if !self.can_copy_history_from(previous) {
            return;
        }
        for (dst, src) in self.pads.iter_mut().zip(&other.pads) {
            dst.runtime.copy_history_from(src.runtime.as_ref());
            if let (Some(dst_delay), Some(src_delay)) = (&mut dst.delay, &src.delay) {
                dst_delay.copy_from(src_delay);
            }
        }
        self.routed = other.routed;
    }

    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let Some(retarget_proof) = proof.as_any().downcast_ref::<DrumRackRetargetProof>() else {
            return false;
        };
        if retarget_proof.rack_id != self.rack_id || retarget_proof.sample_rate != self.sample_rate
        {
            return false;
        }
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        if self.rack_id != other.rack_id
            || self.sample_rate != other.sample_rate
            || self.max_events_per_block != other.max_events_per_block
        {
            return false;
        }
        // Every non-quiescent pad in `other` must exist in `self` with compatible
        // identity, latency compensation, asset slot mapping, and child runtime.
        for src in &other.pads {
            let src_quiescent = src.runtime.is_quiescent()
                && src
                    .delay
                    .as_ref()
                    .is_none_or(DrumCompensationDelay::is_quiet);
            match self.pads.iter().find(|dst| dst.pad_id == src.pad_id) {
                Some(dst) => {
                    if !src_quiescent
                        && (dst.device_id != src.device_id
                            || dst.compensation_delay_frames != src.compensation_delay_frames
                            || dst.asset_slot_map != src.asset_slot_map
                            || !dst.runtime.can_copy_history_from(src.runtime.as_ref()))
                    {
                        return false;
                    }
                }
                None => {
                    if !src_quiescent {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if !self.can_retarget_history_from(previous, proof) {
            return;
        }
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.routed.fill(None);
        let mut dst_routed_cursor = 0_usize;

        for (src_idx, src) in other.pads.iter().enumerate() {
            let Some(dst_idx) = self.pads.iter().position(|dst| dst.pad_id == src.pad_id) else {
                continue;
            };
            let dst = &mut self.pads[dst_idx];
            if dst.device_id == src.device_id
                && dst.compensation_delay_frames == src.compensation_delay_frames
                && dst.asset_slot_map == src.asset_slot_map
                && dst.runtime.can_copy_history_from(src.runtime.as_ref())
            {
                dst.runtime.copy_history_from(src.runtime.as_ref());
                if let (Some(dst_delay), Some(src_delay)) = (&mut dst.delay, &src.delay) {
                    dst_delay.copy_from(src_delay);
                }
                for entry in other.routed.iter().flatten() {
                    if usize::from(entry.pad_index) == src_idx
                        && dst_routed_cursor < self.routed.len()
                    {
                        self.routed[dst_routed_cursor] = Some(RoutedPadOccurrence {
                            occurrence_id: entry.occurrence_id,
                            pad_index: dst_idx as u8,
                            note_off_sent: entry.note_off_sent,
                        });
                        dst_routed_cursor += 1;
                    }
                }
            }
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        let mut total = std::mem::size_of::<Self>()
            .saturating_add(
                self.pads
                    .capacity()
                    .saturating_mul(std::mem::size_of::<PadRuntime>()),
            )
            .saturating_add(
                self.chunk_events
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NativeInstrumentEvent>()),
            );
        for pad in &self.pads {
            total = total
                .saturating_add(pad.runtime.fixed_storage_bytes())
                .saturating_add(
                    pad.event_scratch
                        .capacity()
                        .saturating_mul(std::mem::size_of::<NativeInstrumentEvent>()),
                )
                .saturating_add(
                    pad.asset_slot_map
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(DeviceAssetSlot, DeviceAssetSlot)>()),
                );
            if let Some(delay) = &pad.delay {
                total = total.saturating_add(delay.storage_bytes());
            }
        }
        total
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DrumRackRuntime {
    fn route_single_event(&mut self, event: NativeInstrumentEvent) {
        match event {
            NativeInstrumentEvent::NoteOn {
                frame_offset,
                occurrence_id,
                pitch,
                velocity,
            } => {
                let Some(pad_idx_u8) = self.note_to_pad.get(usize::from(pitch)).copied().flatten()
                else {
                    return;
                };
                let pad_idx = usize::from(pad_idx_u8);
                let Some((choke_group, play_note)) = self
                    .pads
                    .get(pad_idx)
                    .map(|pad| (pad.choke_group, pad.play_note))
                else {
                    return;
                };
                if let Some(group) = choke_group {
                    for slot in &mut self.routed {
                        if let Some(entry) = slot.as_mut()
                            && usize::from(entry.pad_index) != pad_idx
                            && !entry.note_off_sent
                            && let Some(other_pad) = self.pads.get_mut(usize::from(entry.pad_index))
                            && other_pad.choke_group == Some(group)
                        {
                            entry.note_off_sent = true;
                            if other_pad.event_len < other_pad.event_scratch.len() {
                                other_pad.event_scratch[other_pad.event_len] =
                                    NativeInstrumentEvent::NoteOff {
                                        frame_offset,
                                        occurrence_id: entry.occurrence_id,
                                    };
                                other_pad.event_len += 1;
                            }
                        }
                    }
                }

                if let Some(existing) = Self::find_routed(&self.routed, occurrence_id) {
                    self.routed[existing] = Some(RoutedPadOccurrence {
                        occurrence_id,
                        pad_index: pad_idx_u8,
                        note_off_sent: false,
                    });
                } else if let Some(free) = Self::find_free_routed(&self.routed) {
                    self.routed[free] = Some(RoutedPadOccurrence {
                        occurrence_id,
                        pad_index: pad_idx_u8,
                        note_off_sent: false,
                    });
                } else {
                    return;
                }

                if let Some(pad) = self.pads.get_mut(pad_idx)
                    && pad.event_len < pad.event_scratch.len()
                {
                    pad.event_scratch[pad.event_len] = NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch: play_note,
                        velocity,
                    };
                    pad.event_len += 1;
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(slot) = Self::find_routed(&self.routed, occurrence_id)
                    && let Some(entry) = self.routed[slot].as_mut()
                    && !entry.note_off_sent
                {
                    entry.note_off_sent = true;
                    if let Some(pad) = self.pads.get_mut(usize::from(entry.pad_index))
                        && pad.event_len < pad.event_scratch.len()
                    {
                        pad.event_scratch[pad.event_len] = event;
                        pad.event_len += 1;
                    }
                }
            }
            NativeInstrumentEvent::NotePressure { occurrence_id, .. }
            | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
            | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                if let Some(slot) = Self::find_routed(&self.routed, occurrence_id.get())
                    && let Some(entry) = self.routed[slot]
                    && let Some(pad) = self.pads.get_mut(usize::from(entry.pad_index))
                    && pad.event_len < pad.event_scratch.len()
                {
                    pad.event_scratch[pad.event_len] = event;
                    pad.event_len += 1;
                }
            }
            NativeInstrumentEvent::Expression { .. }
            | NativeInstrumentEvent::ControlChange { .. }
            | NativeInstrumentEvent::PitchBend { .. } => {
                for pad in &mut self.pads {
                    if pad.event_len < pad.event_scratch.len() {
                        pad.event_scratch[pad.event_len] = event;
                        pad.event_len += 1;
                    }
                }
            }
        }
    }
}
